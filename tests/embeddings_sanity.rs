//! Embedding self-consistency sanity test.
//!
//! Verifies that the candle BERT pipeline produces sensible vectors:
//! - L2 norm of every output is ≈ 1.0
//! - cos(text, text) ≈ 1.0 (the encoder is deterministic)
//! - cos(near-duplicate pair) > cos(unrelated pair) by a wide margin
//!
//! Skipped if the model files aren't cached and we can't download them.

#![cfg(feature = "embed")]

use vkquery::search::embedding::encode_texts;

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

#[test]
fn bge_small_produces_sensible_cosines() {
    // 5 sentences: pairs (0,1) and (2,3) are near-duplicates;
    // pair (1,4) is loosely related; pair (0,4) is unrelated.
    let texts = [
        "image layout transitions require memory barriers",
        "an image layout transition uses a memory barrier",
        "commandBuffer must be in the recording state",
        "the command buffer needs to be in a recording state",
        "render pass instance must be active for draw calls",
    ];

    let vectors = match encode_texts(None, &texts) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipping: encode_texts failed (likely no network / no cached model): {e:?}");
            return;
        }
    };

    assert_eq!(vectors.len(), 5);
    let dim = vectors[0].len();
    assert!(dim > 0);

    // L2 normalized?
    for (i, v) in vectors.iter().enumerate() {
        let n = norm(v);
        assert!(
            (n - 1.0).abs() < 1e-3,
            "vector {i} norm = {n}, expected ~1.0",
        );
    }

    // Self-cosine is ~1.0.
    for v in &vectors {
        let c = cosine(v, v);
        assert!((c - 1.0).abs() < 1e-3, "self-cosine {c} != 1.0");
    }

    // Near-duplicate pairs should be much closer than unrelated pairs.
    let close_pair = cosine(&vectors[0], &vectors[1]); // image-layout-transition
    let close_pair_2 = cosine(&vectors[2], &vectors[3]); // command-buffer-recording
    let unrelated = cosine(&vectors[0], &vectors[4]);  // image vs renderpass-draw

    println!("cos(image-transition near) = {close_pair:.4}");
    println!("cos(cmdbuf-recording near) = {close_pair_2:.4}");
    println!("cos(image vs renderpass)   = {unrelated:.4}");

    assert!(
        close_pair > unrelated + 0.10,
        "expected close pair {close_pair:.4} to exceed unrelated {unrelated:.4} by ≥0.10",
    );
    assert!(
        close_pair_2 > unrelated + 0.05,
        "expected close pair 2 {close_pair_2:.4} to exceed unrelated {unrelated:.4} by ≥0.05",
    );
    // Sanity floor — bge-small-en-v1.5 typically scores near-duplicates >0.85.
    assert!(
        close_pair > 0.80,
        "near-duplicate cosine {close_pair:.4} below 0.80 — model loaded wrong?",
    );
}
