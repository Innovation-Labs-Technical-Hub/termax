//! Length framing: a 4-byte big-endian length, then that many bytes of
//! UTF-8 JSON.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::ControlError;

/// Largest accepted frame body.
pub const MAX_FRAME: usize = 1024 * 1024;

/// Read one frame. `Ok(None)` on a clean end of stream between frames.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>, ControlError> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(ControlError::Protocol(format!(
            "frame of {len} bytes exceeds the {MAX_FRAME}-byte limit"
        )));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// Write one frame and flush it.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    body: &[u8],
) -> Result<(), ControlError> {
    if body.len() > MAX_FRAME {
        return Err(ControlError::Protocol(format!(
            "frame of {} bytes exceeds the {MAX_FRAME}-byte limit",
            body.len()
        )));
    }
    let len =
        u32::try_from(body.len()).map_err(|_| ControlError::Protocol("frame too large".into()))?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(body).await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_and_eof_between_frames_is_clean() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let writer = tokio::spawn(async move {
            write_frame(&mut a, b"{\"x\":1}").await.unwrap();
            write_frame(&mut a, b"").await.unwrap();
        });
        assert_eq!(read_frame(&mut b).await.unwrap().unwrap(), b"{\"x\":1}");
        assert_eq!(read_frame(&mut b).await.unwrap().unwrap(), b"");
        writer.await.unwrap();
        assert!(read_frame(&mut b).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn oversized_frames_are_refused_without_reading_them() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&(MAX_FRAME as u32 + 1).to_be_bytes())
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut b).await,
            Err(ControlError::Protocol(_))
        ));
        let big = vec![b' '; MAX_FRAME + 1];
        assert!(write_frame(&mut a, &big).await.is_err());
    }

    #[tokio::test]
    async fn a_truncated_frame_is_an_error() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&10u32.to_be_bytes()).await.unwrap();
        a.write_all(b"abc").await.unwrap();
        drop(a);
        assert!(matches!(read_frame(&mut b).await, Err(ControlError::Io(_))));
    }
}
