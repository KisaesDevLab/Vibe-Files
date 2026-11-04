# Help articles — how to write them

These files are the in-app Help (header **Help** button, or F1). Each is compiled into the exe
(`crates/vf-app/src/help.rs` lists them) and readable here as plain Markdown.

## File

- `crates/vf-app/help/<id>.md`, one article per file. The id is fixed by the index in `help.rs`.
- Line 1: `# Title` (the title in the index).
- Then one short paragraph: what the article covers (shown in search results).

## Markdown the Help window draws (nothing else)

- `## Heading` and `### Sub-heading`
- Paragraphs (lines joined until a blank line)
- `- ` bullet lists and `1. ` numbered lists, one level only (no nesting)
- `**bold**` and `` `code` ``; write the exact name of a button, field, tab or menu item in
  `**bold**`, e.g. **Save tape**, and file or folder paths and typed text in `` `code` ``
- `> ` a tip or a caution, one paragraph
- Links to other articles: `[the ten-key tape](help:ten-key)`; only `help:<id>` links, ids from
  the index
- No tables, images, HTML, nested lists or headings below `###`.

## Content rules

- For the people at the firm who use the app: plain words, second person ("you"), short steps.
- Describe only what the app does today. Check every button, field and message name in the
  source (`crates/vf-app/src/**`, Intake screens in `crates/filer-intake/src/ui/**`) and use it
  exactly. Nothing from v2 (users and roles, remote mode, §7216) and nothing removed (check-out).
- Examples use invented names only (`Smith, John (SMIT1001)`, `Baker Farms (1004)`); never real
  client data, never full SSNs or EINs.
- 150 to 600 words.
