# Error index

Each diagnostic code `S0xxx` has an entry here with a title, a minimal failing
example, and the fix. Every code in `topstitch::diagnostic::codes::ALL` must have
a file with `## Example` and `## Fix` sections and a row in the table below
(checked by tests). `tests/diagnostics/examples.rs` runs each example: the
`systemverilog` blocks become the sources `cfg` lists, the Rust statements it
runs must appear in the `rust` blocks, and it must report the entry's code. The
output shown must be what is rendered: by the example for S0301 and S0302, by
the snapshot tests in `tests/diagnostics/typed.rs` for S0201 to S0204.

| Code | Title |
| --- | --- |
| [S0101](S0101.md) | unknown parameter |
| [S0102](S0102.md) | Slang elaboration error |
| [S0103](S0103.md) | unsupported port form |
| [S0104](S0104.md) | interface port unsupported |
| [S0105](S0105.md) | unsupported override expression |
| [S0201](S0201.md) | incompatible types |
| [S0202](S0202.md) | no such field |
| [S0203](S0203.md) | index out of range |
| [S0204](S0204.md) | field access through union |
| [S0301](S0301.md) | module defined twice in the build |
| [S0302](S0302.md) | macro defined differently in the build |
