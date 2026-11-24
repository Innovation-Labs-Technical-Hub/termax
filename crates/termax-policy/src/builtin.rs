//! Built-in rules that user rules can never relax.

use crate::analysis::{SimpleCommand, Word};
use crate::rules::Effect;

/// Device files that are safe to write to.
const BENIGN_DEVICES: &[&str] = &["/dev/null", "/dev/stdout", "/dev/stderr", "/dev/tty"];

/// `/dev/` name prefixes of block storage devices.
const BLOCK_DEVICE_PREFIXES: &[&str] = &[
    "sd", "hd", "vd", "xvd", "nvme", "mmcblk", "disk", "md", "dm-", "mapper/",
];

/// Programs whose built-in checks depend on argument values, so run-time
/// expansions in their arguments cannot be trusted.
const ARG_SENSITIVE: &[&str] = &["rm", "chmod", "chown", "chgrp", "dd", "tee"];

/// Built-in findings for one command. Multiple findings may apply.
pub(crate) fn check_command(cmd: &SimpleCommand) -> Vec<(Effect, String)> {
    let mut out = Vec::new();
    let name = cmd.name();
    let deny = |out: &mut Vec<(Effect, String)>, r: String| out.push((Effect::Deny, r));

    if name.starts_with("mkfs") {
        deny(&mut out, format!("`{name}` formats a filesystem"));
    }
    match name {
        "rm" => check_rm(cmd, &mut out),
        "chmod" | "chown" | "chgrp" => {
            let flags = Flags::parse(&cmd.args);
            if flags.has_short('R') || flags.has_long("recursive") {
                for target in flags.operands.iter().filter(|t| is_critical_path(t)) {
                    deny(&mut out, format!("recursive `{name}` on `{target}`"));
                }
            }
        }
        "dd" => {
            for arg in &cmd.args {
                if let Some(dev) = arg.text.strip_prefix("of=")
                    && is_harmful_device(dev)
                {
                    deny(&mut out, format!("`dd` writes to device `{dev}`"));
                }
            }
        }
        "tee" => {
            let flags = Flags::parse(&cmd.args);
            for target in flags.operands.iter().filter(|t| is_block_device(t)) {
                deny(&mut out, format!("`tee` writes to block device `{target}`"));
            }
        }
        "shutdown" | "reboot" | "halt" | "poweroff" => {
            out.push((Effect::Ask, format!("`{name}` changes host power state")));
        }
        "systemctl" => {
            if cmd.args.iter().any(|a| {
                matches!(
                    a.text.as_str(),
                    "reboot" | "poweroff" | "halt" | "kexec" | "soft-reboot"
                )
            }) {
                out.push((Effect::Ask, "`systemctl` changes host power state".into()));
            }
        }
        "init" | "telinit"
            if cmd
                .args
                .iter()
                .any(|a| matches!(a.text.as_str(), "0" | "6")) =>
        {
            out.push((Effect::Ask, format!("`{name}` changes host power state")));
        }
        _ => {}
    }
    if ARG_SENSITIVE.contains(&name) && cmd.args.iter().any(|a| a.dynamic) {
        out.push((
            Effect::Ask,
            format!("`{name}` arguments contain run-time expansions"),
        ));
    }
    out
}

/// Built-in finding for a redirection write target.
pub(crate) fn check_write_target(target: &Word) -> Option<(Effect, String)> {
    if is_block_device(&target.text) {
        return Some((
            Effect::Deny,
            format!("redirect writes to block device `{}`", target.text),
        ));
    }
    if target.dynamic {
        return Some((
            Effect::Ask,
            format!("redirect target `{}` is computed at run time", target.text),
        ));
    }
    None
}

fn check_rm(cmd: &SimpleCommand, out: &mut Vec<(Effect, String)>) {
    let flags = Flags::parse(&cmd.args);
    if flags.has_long("no-preserve-root") {
        out.push((Effect::Deny, "`rm --no-preserve-root`".into()));
    }
    // Non-interactive `rm -r` deletes without prompting, so `-f` is not
    // required for the deletion to be catastrophic.
    let recursive = flags.has_short('r') || flags.has_short('R') || flags.has_long("recursive");
    if recursive {
        for target in flags.operands.iter().filter(|t| is_critical_path(t)) {
            out.push((Effect::Deny, format!("recursive delete of `{target}`")));
        }
    }
}

/// Parsed option flags and operands of a command (GNU conventions).
struct Flags<'a> {
    short: String,
    long: Vec<&'a str>,
    operands: Vec<&'a str>,
}

impl<'a> Flags<'a> {
    fn parse(args: &'a [Word]) -> Self {
        let mut flags = Flags {
            short: String::new(),
            long: Vec::new(),
            operands: Vec::new(),
        };
        let mut only_operands = false;
        for arg in args {
            let t = arg.text.as_str();
            if only_operands {
                flags.operands.push(t);
            } else if t == "--" {
                only_operands = true;
            } else if let Some(long) = t.strip_prefix("--") {
                flags.long.push(long.split('=').next().unwrap_or(long));
            } else if t.len() > 1 && t.starts_with('-') {
                flags.short.push_str(&t[1..]);
            } else {
                flags.operands.push(t);
            }
        }
        flags
    }

    fn has_short(&self, c: char) -> bool {
        self.short.contains(c)
    }

    fn has_long(&self, name: &str) -> bool {
        self.long.contains(&name)
    }
}

/// True for `/`, `/*`, `~`, `~/*`, `$HOME`, `${HOME}/*` and lexical
/// equivalents such as `//`, `/./` or `/..`.
fn is_critical_path(path: &str) -> bool {
    let rest = if let Some(r) = path.strip_prefix('/') {
        r
    } else if let Some(r) = ["~", "$HOME", "${HOME}"]
        .iter()
        .find_map(|home| path.strip_prefix(home))
    {
        if !(r.is_empty() || r.starts_with('/')) {
            return false;
        }
        r
    } else {
        return false;
    };
    // A `*` directly under the root expands to every top-level entry.
    let mut depth = 0usize;
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            "*" if depth == 0 => {}
            ".." => depth = depth.saturating_sub(1),
            _ => depth += 1,
        }
    }
    depth == 0
}

fn is_block_device(path: &str) -> bool {
    path.strip_prefix("/dev/")
        .is_some_and(|dev| BLOCK_DEVICE_PREFIXES.iter().any(|p| dev.starts_with(p)))
}

fn is_harmful_device(path: &str) -> bool {
    path.starts_with("/dev/") && !BENIGN_DEVICES.contains(&path) && !path.starts_with("/dev/fd/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_paths() {
        for p in [
            "/",
            "//",
            "/.",
            "/..",
            "/*",
            "/./*",
            "~",
            "~/",
            "~/*",
            "$HOME",
            "$HOME/",
            "${HOME}/*",
        ] {
            assert!(is_critical_path(p), "{p} should be critical");
        }
        for p in [
            "./build",
            "build",
            "/tmp/x",
            "/*/x",
            "~/project",
            "$HOMEDIR",
            "$HOME/x",
            "*",
            "",
        ] {
            assert!(!is_critical_path(p), "{p} should not be critical");
        }
    }

    #[test]
    fn block_devices() {
        assert!(is_block_device("/dev/sda"));
        assert!(is_block_device("/dev/nvme0n1p1"));
        assert!(is_block_device("/dev/disk2"));
        assert!(!is_block_device("/dev/null"));
        assert!(!is_harmful_device("/dev/null"));
        assert!(!is_harmful_device("/dev/fd/1"));
        assert!(is_harmful_device("/dev/sdb"));
    }
}
