# Fuzz targets

ClusterFuzzLite fuzzes these weekly in batch, an hour after pruning the stored corpus, and either workflow can be run by hand. They reach refquiem's readers through the `fuzzing` feature.

- `rust` reads the comments, doc comments and string literals of a Rust file and locates every mention the way a scan does.
- `markdown` does the same for a Markdown file.
- `declarations` reads the declarations of a Rust file the way indexing does.

```sh
cargo +nightly fuzz run rust fuzz/corpus/rust fuzz/seeds/rust -- -dict=fuzz/refquiem.dict
```

## Seed Corpus

`fuzz/seeds/<target>/` holds one input per file, grown from refquiem's own sources, README and test fixtures and reduced with `-set_cover_merge=1`. `.clusterfuzzlite/build.sh` zips it into the starting corpus and fails a target without seeds.
