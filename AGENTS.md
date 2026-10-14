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
- Designs and generators never build SystemVerilog text by hand; extend the model and the emitter instead.
- Run `cargo xtask ci` from the repository root before finishing (fmt, workspace-wide clippy `-D warnings`, hygiene, library boundary, tests). Run it inside `nix develop`, the pinned environment that CI also uses; tools come from `flake.nix`, never from distribution packages. The first build compiles the vendored Slang (minutes); ccache makes rebuilds instant. `cargo xtask explain <CODE>` shows a diagnostic's documentation.
- Ask before adding new tooling or CI.

## Commits

- Use [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): summary`, e.g. `feat(import): resolve packed struct ports`. Types: `feat`, `fix`, `docs`, `refactor`, `test`, `perf`, `build`, `ci`, `chore`.
- Reference issues in the body (`Refs #N`; `Closes #N` only when acceptance criteria are met).
- No AI or assistant attribution (`Co-Authored-By`, "Generated with") in commits, PRs or issues.
- Prefer linear history: rebase branches onto `main` and land PRs by rebase or fast-forward, without merge commits. Port upstream Topstitch fixes by hand and reference the upstream commit (`xlsynth/topstitch@<sha>`) in the commit body; tag merges no longer apply since the paths diverged. See `doc/fork.md`.

## Licensing

- Apache-2.0. Code forked from Topstitch keeps its upstream copyright and notices.
