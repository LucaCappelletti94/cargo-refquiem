//! The names a SQL file declares, read as head's `.sql` files are.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(source) = std::str::from_utf8(data) {
        let source = source.to_owned();
        refquiem_fuzz::on_large_stack(move || refquiem::fuzzing::sql_declarations(&source));
    }
});
