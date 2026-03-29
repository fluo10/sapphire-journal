# sapphire-journal-core

Core library for [sapphire-journal](https://github.com/fluo10/sapphire-journal) — a Markdown-based task and note manager.

## What this crate provides

- **Data model** — `Entry`, `Frontmatter`, `TaskMeta`, `EventMeta`
- **Parser / serializer** — reads and writes `.md` files with YAML frontmatter (`parse_entry`, `read_entry`, `render_entry`, `write_entry`)
- **Journal detection** — `Journal::find()` walks up the directory tree to locate `.sapphire-journal/`, the same way `git` finds `.git/`
- **Filename helpers** — `slugify`, `entry_filename`, `new_entry_path` for the `{year}/{caretta-id}_{slug}.md` layout

## License

MIT OR Apache-2.0
