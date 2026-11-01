# Markdown test sources

These are fictional data for testing v0.1 indexing and search. Feature descriptions
are not evidence of application implementation.

## Source registration

| Project | Source root relative to the repository root | Documents |
| --- | --- | --- |
| banking | `tests/fixtures/banking` | `authentication.md`, `transfers.md`, `audit.md` |
| storefront | `tests/fixtures/storefront` | `authentication.md`, `catalog.md`, `orders.md` |

Register the two directories separately. Their parent, `tests/fixtures`, is not
a source: this README must not enter the index. Each source is bound only to its
own project. There are six documents in total; the chunk count depends on the
chosen heading-based splitting strategy.

Every document has a unique stable `id` in YAML frontmatter. The frontmatter ID
belongs to the document, not the logical Memory. Dates, indexing times and hashes
are computed by the indexer rather than taken from the data.

## Search expectations

Checks compare document sets, not exact chunk counts, ranking order or full-text
frontmatter search. Search markers appear in the body text.

| Authorized projects | Query | Expected documents |
| --- | --- | --- |
| banking | `OAuth2` | only `banking/authentication.md` |
| storefront | `OAuth2` | only `storefront/authentication.md` |
| banking, storefront (explicitly authorized global search) | `OAuth2` | both `authentication.md` files |
| banking (global search authorized only for banking) | `OAuth2` | only `banking/authentication.md` |
| banking | `shopauthmarker` | none |
| storefront | `bankauthmarker` | none |
| banking | `banktransfermarker` | only `banking/transfers.md` |
| banking | `bankauditmarker` | only `banking/audit.md` |
| storefront | `shopcatalogmarker` | only `storefront/catalog.md` |
| storefront | `shopordermarker` | only `storefront/orders.md` |

Directly getting another project's document must also be denied. The global
search parameter does not itself expand permissions. Each search result is
checked for its path and a valid source-file line range; numbering starts at the
first frontmatter line.

## Index change scenarios

Run each scenario on a separate temporary copy of these directories and a
separate test SQLite database. Keep the original fixtures unchanged.

1. **Reindexing.** Index both sources twice without changes. Six documents remain,
   chunks are not duplicated and content hashes stay unchanged.
2. **Update.** In the copy of `banking/transfers.md`, replace `banktransfermarker`
   with `bankupdatedmarker`, preserving the frontmatter ID. After indexing, the
   old marker is absent and the new marker is found only in banking; storefront
   is unchanged. The document and FTS update in one transaction.
3. **Deletion.** Delete `storefront/catalog.md` in the copy, then complete source
   reconciliation successfully. The document, its chunks and FTS records disappear;
   `shopcatalogmarker` is absent. The other five documents remain available.
4. **Rename.** Rename `banking/transfers.md` to `banking/payments.md` in the copy
   without changing content or ID. After indexing, the new path is available,
   the old path is absent and no duplicate document exists.
5. **Recovery.** Close connections, delete only the temporary SQLite index and
   its temporary WAL/SHM files, create the schema and reindex the original fixtures.
   Six documents and the expected search results are restored. Register
   project/source bindings again unless they are stored separately from SQLite.
6. **Interruption.** Roll back the transaction after replacing document chunks
   but before completing the index update. The previous consistent document and
   FTS version remains; rerunning applies the update successfully.

The CLI and basic indexer are implemented. Automated tests cover isolation,
line numbers, reindexing, updates, deletion, rollback on failure and database
recovery. Stable frontmatter IDs preserve document identity across renames;
documents without stable IDs are tracked by path.
