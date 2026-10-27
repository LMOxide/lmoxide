# Archived data files

The game's data used to live in these files; it now lives in the content tree (`content/`,
docs/CONTENT.md), and the server reads only that. They are kept as the record of what the
extractors in `tools/re/` produced from the client and the live captures:

- `lmoxide-content import` reads them (and the reference catalogs in `data/`) to build a content
  tree from scratch (it overwrites the whole tree);
- the tests that check the content tree against them (`same_as_the_old_files` in the server,
  the API and lmoxide-content) and `tools/content/verify_import.py`;
- the extractors in `tools/re/` write here when run again.

Edit the content tree, not these files: changes here reach the game only through a new import.
