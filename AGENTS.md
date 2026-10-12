# Agent instructions

Sandstitch is an executable hardware specification library. `doc/product.md` is the canonical product and architecture plan; read it before proposing design changes.

## Work tracking

- Track durable work in GitHub issues, sub-issues, dependencies, milestones and PRs using `gh`.
- Before adding persistent plans, TODO lists, or custom lane/slice/spike identifiers, search existing issues and reuse them. Detailed execution plans may be temporary; they are not the backlog.
- Link settled design decisions from issues to canonical docs in `doc/`.
- Never create, edit or close GitHub issues without authorization.

## Engineering

- Rust first. Designs and generators are authored in Rust; language bindings are deferred.
- One authoritative model: the forked composition model owns structure; the resolved snapshot is read-only and shared by RTL emission, the explorer and documentation. Do not add a second mutable graph or hand-duplicate the snapshot schema.
- Keep changes small and coherent. Refactor the affected area first, with behavior-preserving tests, when structure blocks a change. No unrelated cleanup.
- Fail explicitly: unsupported SystemVerilog constructs produce diagnostics; never silently drop ports.
- Use the repository's native checks (`cargo fmt`, `cargo clippy`, `cargo test` once code exists) and run them on changed areas. Ask before adding new tooling or CI.

## Commits

- Use [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): summary`, e.g. `feat(import): resolve packed struct ports`. Types: `feat`, `fix`, `docs`, `refactor`, `test`, `perf`, `build`, `ci`, `chore`.
- Reference issues in the body (`Refs #N`; `Closes #N` only when acceptance criteria are met).

## Licensing

- Apache-2.0. Code forked from Topstitch keeps its upstream copyright and notices.
