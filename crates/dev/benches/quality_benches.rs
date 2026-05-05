use core::hint::black_box;
use criterion::{criterion_group, criterion_main, Criterion};
use shared_utils::image_detection::PrecisionMetadata;
use shared_utils::image_quality_detector::analyze_image_quality;

fn bench_quality_analysis(c: &mut Criterion) {
    let width = 1000u32;
    let height = 1000u32;
    let rgba_data = vec![128u8; (width * height * 4) as usize];

    c.bench_function("analyze_image_quality_1k", |b| {
        b.iter(|| {
            analyze_image_quality(
                black_box(width),
                black_box(height),
                black_box(&rgba_data),
                black_box(4_000_000),
                black_box("PNG"),
                black_box(1),
                black_box(PrecisionMetadata::default()),
            )
        });
    });
}

criterion_group!(benches, bench_quality_analysis);
criterion_main!(benches);
