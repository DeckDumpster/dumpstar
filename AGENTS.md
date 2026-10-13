# Agent Instructions

Read `CLAUDE.md` first: it describes the repository layout, what is not here
yet, and how to build and test.

## Non-interactive shell commands

Use non-interactive flags with file operations (`cp -f`, `mv -f`, `rm -f`,
`rm -rf`) so an aliased `-i` cannot hang the session.

## Tests

Run the unit tests of the crate you changed (`cargo test -p <crate>`). The
container gates are run by CI through `deploy/ci.sh`, not by hand.
