//! The standard library paths and the English, SQL, WASI and WebAssembly words that
//! `cargo xtask vocab` builds.

/// The public paths reachable from one crate root.
pub(crate) struct Layer {
    /// The root the layer's paths start with.
    pub(crate) root: &'static str,
    /// The suffixes, of two or more segments, of every path.
    suffixes: fst::Set<&'static [u8]>,
    /// Every segment of every path.
    names: fst::Set<&'static [u8]>,
}

impl Layer {
    fn new(root: &'static str, suffixes: &'static [u8], names: &'static [u8]) -> Self {
        let set = |bytes| fst::Set::new(bytes).expect("data/std holds fst sets written by xtask");
        Self {
            root,
            suffixes: set(suffixes),
            names: set(names),
        }
    }

    /// Whether `path`, spelled with `::`, ends a public path of this layer.
    pub(crate) fn ends_a_path(&self, path: &str) -> bool {
        self.suffixes.contains(path)
    }

    /// Whether some public path of this layer has a segment `name`.
    pub(crate) fn has_name(&self, name: &str) -> bool {
        self.names.contains(name)
    }
}

/// The `core`, `alloc`, `std` and `proc_macro` layers.
pub(crate) fn layers() -> [Layer; 4] {
    [
        Layer::new(
            "core",
            include_bytes!("../data/std/core.fst"),
            include_bytes!("../data/std/core-names.fst"),
        ),
        Layer::new(
            "alloc",
            include_bytes!("../data/std/alloc.fst"),
            include_bytes!("../data/std/alloc-names.fst"),
        ),
        Layer::new(
            "std",
            include_bytes!("../data/std/std.fst"),
            include_bytes!("../data/std/std-names.fst"),
        ),
        Layer::new(
            "proc_macro",
            include_bytes!("../data/std/proc_macro.fst"),
            include_bytes!("../data/std/proc_macro-names.fst"),
        ),
    ]
}

/// Words a bare name may simply be, English words in running text and SQL key words, WASI
/// functions and WebAssembly instructions in running text and code spans.
pub(crate) struct Vocabulary {
    words: fst::Set<&'static [u8]>,
}

impl Vocabulary {
    pub(crate) fn english() -> Self {
        Self {
            words: fst::Set::new(&include_bytes!("../data/english/words.fst")[..])
                .expect("data/english holds an fst set written by xtask"),
        }
    }

    /// The reserved key words of SQL:2023, uppercase as the standard spells them.
    pub(crate) fn sql_2023() -> Self {
        Self {
            words: fst::Set::new(&include_bytes!("../data/sql/words.fst")[..])
                .expect("data/sql holds an fst set written by xtask"),
        }
    }

    /// The function names of WASI Preview 1.
    pub(crate) fn wasi_preview1() -> Self {
        Self {
            words: fst::Set::new(&include_bytes!("../data/wasi/words.fst")[..])
                .expect("data/wasi holds an fst set written by xtask"),
        }
    }

    /// The WebAssembly 3.0 instructions whose text-format name has no dot.
    pub(crate) fn wasm_3() -> Self {
        Self {
            words: fst::Set::new(&include_bytes!("../data/wasm/words.fst")[..])
                .expect("data/wasm holds an fst set written by xtask"),
        }
    }

    /// The compound class names and camelCase method names of the public Android 16 API.
    pub(crate) fn android_36() -> Self {
        Self {
            words: fst::Set::new(&include_bytes!("../data/android/words.fst")[..])
                .expect("data/android holds an fst set written by xtask"),
        }
    }

    /// Whether `word`, spelled as the vocabulary stores it, is one of its words.
    pub(crate) fn contains(&self, word: &str) -> bool {
        self.words.contains(word)
    }
}
