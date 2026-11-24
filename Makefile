# Termax tasks live in the justfile; this Makefile forwards to it so
# `make test`, `make perf`, ... work too. Arguments go in ARGS:
#   make test ARGS=termax-ssh
#   make perf ARGS="--no-ssh"

JUST := $(shell command -v just 2>/dev/null)
ARGS ?=

.DEFAULT_GOAL := help

help:
ifndef JUST
	@echo "This project's tasks need 'just' (https://just.systems): cargo install just"
	@exit 1
else
	@just --list --unsorted
endif

%:
ifndef JUST
	@echo "This project's tasks need 'just' (https://just.systems): cargo install just"
	@exit 1
else
	@just $@ $(ARGS)
endif

.PHONY: help
