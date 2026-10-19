//! Public paths of the standard library crates, built by `cargo xtask vocab`.

/// The suffixes, of two or more segments, of every public path reachable from one crate root.
pub(crate) struct Layer {
    /// The root the layer's paths start with.
    pub(crate) root: &'static str,
    suffixes: fst::Set<&'static [u8]>,
}

impl Layer {
    fn new(root: &'static str, bytes: &'static [u8]) -> Self {
        Self {
            root,
            suffixes: fst::Set::new(bytes).expect("data/std holds fst sets written by xtask"),
        }
    }

    /// Whether `path`, spelled with `::`, ends a public path of this layer.
    pub(crate) fn ends_a_path(&self, path: &str) -> bool {
        self.suffixes.contains(path)
    }
}

/// The `core`, `alloc`, `std` and `proc_macro` layers.
pub(crate) fn layers() -> [Layer; 4] {
    [
        Layer::new("core", include_bytes!("../data/std/core.fst")),
        Layer::new("alloc", include_bytes!("../data/std/alloc.fst")),
        Layer::new("std", include_bytes!("../data/std/std.fst")),
        Layer::new("proc_macro", include_bytes!("../data/std/proc_macro.fst")),
    ]
}
