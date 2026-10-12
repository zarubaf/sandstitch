# Error index

Each diagnostic code `S0xxx` has an entry here with a title, a minimal failing
example, and the fix. Every code in `topstitch::diagnostic::codes::ALL` must have
a file with `## Example` and `## Fix` sections and a row in the table below
(checked by tests). `tests/diagnostics/examples.rs` runs each example: the
`systemverilog` blocks become the sources `cfg` lists, the statements it runs
must be exactly those of the `rust` blocks (ignoring whitespace and comments),
and it must report the entry's code. Where an entry shows the output, its
header, notes and helps must be those of the run and the source lines it
quotes must be in the example (S0206, S0301 and S0302: the whole output).

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
| [S0205](S0205.md) | field access on an unpacked struct |
| [S0206](S0206.md) | unpacked aggregate connected as bits |
| [S0301](S0301.md) | module defined twice in the build |
| [S0302](S0302.md) | macro defined differently in the build |
