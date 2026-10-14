//! The comments, doc comments and string literals of a Rust file, read as a scan reads them.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(
    init: refquiem_fuzz::tolerate_pulldown_cmark_panics(),
    |data: &[u8]| {
        if let Ok(source) = std::str::from_utf8(data) {
            let source = source.to_owned();
            refquiem_fuzz::on_large_stack(move || refquiem::fuzzing::mentions(&source, true));
        }
    }
);
