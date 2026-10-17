//! Synthetic parser-only benchmark. Run with `cargo run --release -p rubylight-protocol
//! --example pyrowave_records_bench -- [iterations]`. This does not measure decoder or GPU work.
use rubylight_protocol::pyrowave::{
    Records, PACKET_CONTIGUOUS, PACKET_FRAME_END, PACKET_FRAME_START, PACKET_RECORD_START,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Instant,
};

struct CountAllocations;
static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static REALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountAllocations {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            REALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        System.realloc(ptr, layout, size)
    }
}

#[global_allocator]
static ALLOCATOR: CountAllocations = CountAllocations;

fn frame(count: usize, record_bytes: usize, sequence: u32, shuffled: bool, gaps: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(count * record_bytes);
    for i in 0..count {
        // Counts are powers of two, so this odd multiplier permutes all record indices.
        let index = if shuffled { (i * 769) % count } else { i };
        let start = body.len();
        body.resize(start + record_bytes, i as u8);
        body[start..start + 4].copy_from_slice(&((sequence << 28) | ((record_bytes as u32 / 4) << 16)).to_le_bytes());
        body[start + 4..start + 8].copy_from_slice(&((index as u32) << 8).to_le_bytes());
    }
    // Each 1024-byte payload starts at a record boundary, including after a lost packet.
    let packet_count = body.len() / 1024;
    let mut data = vec![0; 4];
    let mut previous = None;
    for (i, packet) in body.chunks(1024).enumerate() {
        if gaps && i % 64 == 31 {
            continue;
        }
        let mut flags = PACKET_RECORD_START;
        if previous == i.checked_sub(1) && i != 0 {
            flags |= PACKET_CONTIGUOUS;
        }
        if i == 0 { flags |= PACKET_FRAME_START; }
        if i + 1 == packet_count { flags |= PACKET_FRAME_END; }
        data.extend_from_slice(&((packet.len() + if i == 0 { 16 } else { 0 }) as u32).to_le_bytes());
        data.extend_from_slice(&flags.to_le_bytes());
        if i == 0 {
            data.extend_from_slice(&[1, 0, 0, 0, 0, 4, 0, 0]); // Final payload is 1024 bytes.
            data.extend_from_slice(&(0x8000_0000 | (sequence << 28) | 2183 | (1967 << 14)).to_le_bytes());
            data.extend_from_slice(&(count as u32).to_le_bytes());
        }
        data.extend_from_slice(packet);
        previous = Some(i);
    }
    data
}

fn main() {
    let iterations = std::env::args().nth(1).map(|s| s.parse::<usize>().unwrap()).unwrap_or(250);
    assert!(iterations != 0);
    println!("parser-only; {iterations} iterations; allocation counts are measured separately after warmup");
    for (count, bytes) in [(1024, 128), (4096, 256), (8192, 512)] {
        for (name, shuffled, gaps) in [("ordered", false, false), ("shuffled", true, false), ("loss 1/64", true, true)] {
            let full = frame(count, bytes, 0, shuffled, false);
            let frames = [frame(count, bytes, 1, shuffled, gaps), frame(count, bytes, 2, shuffled, gaps)];
            let mut records = Records::default();
            let consume = |packet: &[u8]| {
                black_box((packet.len(), packet[0]));
                true
            };
            assert!(records.push_frame(&full, consume));
            for i in 0..8 { assert!(records.push_frame(&frames[i % 2], consume)); }
            let start = Instant::now();
            for i in 0..iterations {
                assert!(records.push_frame(black_box(&frames[i % 2]), consume));
            }
            let micros = start.elapsed().as_secs_f64() * 1_000_000.0 / iterations as f64;
            ALLOCATIONS.store(0, Ordering::Relaxed);
            REALLOCATIONS.store(0, Ordering::Relaxed);
            COUNTING.store(true, Ordering::Relaxed);
            let ok = records.push_frame(&frames[0], consume);
            COUNTING.store(false, Ordering::Relaxed);
            assert!(ok);
            println!("{:4} KiB / {count:4} records / {name:9}: {micros:8.2} us/frame; allocations={} reallocations={}",
                count * bytes / 1024, ALLOCATIONS.load(Ordering::Relaxed), REALLOCATIONS.load(Ordering::Relaxed));
        }
    }
}
