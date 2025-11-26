use std::{hint::black_box, time::Instant};

use ring::digest::{self, Context};

const ITER: u32 = 100;
const MB: usize = usize::pow(2, 20);

fn main() {
    let data = (0..(128 * MB))
        .map(|i| (i % 256) as u8)
        .collect::<Vec<u8>>();

    let s = Instant::now();
    for _ in 0..ITER {
        let mut ctx = Context::new(&digest::SHA256);
        for chunk in data.chunks(MB) {
            ctx.update(chunk);
        }
        black_box(ctx.finish());
    }
    let d = s.elapsed();
    println!("Done in {:?}, {:?}/iter", d, d / ITER);
}
