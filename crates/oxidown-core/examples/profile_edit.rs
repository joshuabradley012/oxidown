//! Rough breakdown of where apply_edit time goes on a dense ~120KB doc.
//! Run: cargo run --release -p oxidown-core --example profile_edit
use std::time::Instant;

use oxidown_core::parser;

fn main() {
    let doc: String =
        "## Sec\n\nLorem **ipsum** dolor *sit* `code` \u{4f60}\u{597d} \u{1F600}\n\n".repeat(2200);
    println!("doc: {} bytes", doc.len());

    // pulldown event walk only (no node extraction).
    let t = Instant::now();
    let mut count = 0usize;
    for _ in 0..50 {
        count += pulldown_cmark::Parser::new_ext(&doc, pulldown_cmark::Options::empty())
            .into_offset_iter()
            .count();
    }
    println!(
        "pulldown walk: {:.0}us/parse ({} events)",
        t.elapsed().as_secs_f64() * 1e6 / 50.0,
        count / 50
    );

    // Full overlay extraction.
    let t = Instant::now();
    let mut nodes = 0usize;
    for _ in 0..50 {
        nodes = parser::parse(&doc).len();
    }
    println!(
        "parser::parse: {:.0}us/parse ({nodes} nodes)",
        t.elapsed().as_secs_f64() * 1e6 / 50.0
    );

    // Rope to_string cost.
    let rope = ropey::Rope::from_str(&doc);
    let t = Instant::now();
    let mut total = 0usize;
    for _ in 0..50 {
        total += rope.to_string().len();
    }
    println!(
        "rope.to_string: {:.0}us ({} bytes)",
        t.elapsed().as_secs_f64() * 1e6 / 50.0,
        total / 50
    );
}
