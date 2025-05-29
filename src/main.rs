use miden_lib;
use std::fs::File;

use std::io::{stdout, Write};

use curl::easy::Easy;

fn main() {
    let mut file = File::create("foo.tar.gz").unwrap();

    // Download the latest version of the Miden VM
    let mut easy = Easy::new();
    easy.url("https://github.com/0xMiden/miden-vm/archive/refs/tags/v0.14.0.tar.gz")
        .unwrap();
    easy.follow_location(true).unwrap();
    easy.write_function(move |data| {
        file.write_all(data).unwrap();
        Ok(data.len())
    })
    .unwrap();
    easy.perform().unwrap();
}
