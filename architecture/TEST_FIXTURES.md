# Test fixtures are Rust (sp-7y6mzt)

**Decision.** A test fixture — a JWKS/Access issuer, an alert sink, an upstream
stub, a backup check — is a Rust binary or library in this workspace, built by
the same `cargo` the service is built with. A test needs nothing the service
itself does not: no Python, no PyJWT, no `cryptography`. The shell harnesses
under `tests/` keep orchestrating; they build and exec the Rust fixture.

**Why.** The Python fixtures were a second deployment of one service. They failed
PR 77 after 49 minutes when the runner lacked PyJWT, and dumpstar inherited all of
them in one copy (ec394bd). Every runner already has the Rust toolchain.

**How, per fixture.**

| Fixture | Provided by |
| --- | --- |
| Access/JWKS issuer | `dumpstar-test-jwks` (`crates/dumpstar-cli`, feature `test-jwks`, over the kit's `TestAccessFixture`), run through `tests/lib/test-jwks.sh` (sp-w2ng0s). |
| Alert sink, upstream stub, other HTTP stubs | A bin or `#[cfg(test)]` module on `tokio`/`axum`, already workspace dependencies, behind a feature like `test-jwks`, so the shipped image never carries it. Prefer an in-process Rust test over a harness when the test is Rust-reachable. |
| Lake backup check | The production lake is Java by choice (the Nessie dependency, Ryan 2026-10-15); it is out of scope. Its checks are JVM tests, not Python. |
| JSON / port helpers in shell | `jq` and bash (`/dev/tcp`), as `tests/lib/ports.sh` does. |

**Restoring deleted tests.** Only a test that was the sole cover of a use case is
restored (Ryan 2026-10-16: coverage-positive, judged by use case, not code
coverage); the other children close as not needed.

**Enforced by** `tests/ci/no_python_test.sh`: no committed `*.py`, and no
`python3` in `tests/`. Remaining production uses (`deploy/setup-tenant-zone.sh`,
runner base packages) are listed in that test's allowlist and shrink only.
