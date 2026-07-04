//! Scratch tool to inspect pulldown-cmark 0.13's `into_offset_iter()` span semantics.
//! Not part of the shipped crate surface; kept only to document what we learned
//! (see README "Parser span notes"). Run with: cargo run --example span_spike
use pulldown_cmark::{Options, Parser};

fn dump(src: &str) {
    println!("=== {:?} ===", src);
    let parser = Parser::new_ext(src, Options::empty());
    for (event, range) in parser.into_offset_iter() {
        println!(
            "  {:>28} {:?}  => {:?}",
            format!("{:?}", event),
            range.clone(),
            &src[range]
        );
    }
}

fn main() {
    dump("# Title\n");
    dump("##Title\n");
    dump("## \n");
    dump("**bold**\n");
    dump("__bold__\n");
    dump("*em*\n");
    dump("_em_\n");
    dump("***bold-italic***\n");
    dump("**bold *italic* bold**\n");
    dump("`code`\n");
    dump("Title\n===\n");
    dump("Title\n---\n");
    dump("pre **bo\u{1F600}ld** post\n");
    dump("**bold**\n\nnext para *em*\n");
    dump("* not a heading\n");
    dump("``code with ` inside``\n");
    dump("` code `\n");
    dump("``` code ```\n");
    dump("`` `code` ``\n");
    dump("#Not a heading\n");
    dump("####### too many\n");
    dump("  ## indented heading\n");
    dump("# \n");
    dump("#\n");
}
