//! 验证字符约束、长度边界、采样分布与随机源失败路径。

use super::*;

#[test]
fn lengths_and_enabled_groups_are_enforced() {
    for length in [MIN_LENGTH, 20, MAX_LENGTH] {
        for numbers in [false, true] {
            for symbols in [false, true] {
                let options = Options {
                    length,
                    numbers,
                    symbols,
                };
                for _ in 0..16 {
                    let text = generate(options).unwrap();
                    assert_eq!(text.len(), length);
                    assert!(text.bytes().any(|b| LOWER.contains(&b)));
                    assert!(text.bytes().any(|b| UPPER.contains(&b)));
                    assert_eq!(text.bytes().any(|b| DIGITS.contains(&b)), numbers);
                    assert_eq!(text.bytes().any(|b| SYMBOLS.contains(&b)), symbols);
                    assert!(text.bytes().all(|b| LOWER.contains(&b)
                        || UPPER.contains(&b)
                        || (numbers && DIGITS.contains(&b))
                        || (symbols && SYMBOLS.contains(&b))));
                }
            }
        }
    }
}

#[test]
fn rejects_invalid_lengths_before_requesting_randomness() {
    for length in [0, MIN_LENGTH - 1, MAX_LENGTH + 1, usize::MAX] {
        assert_eq!(
            generate_with(
                Options {
                    length,
                    ..Options::default()
                },
                |_| panic!("无效配置不应调用随机源")
            ),
            Err(Error::InvalidLength)
        );
    }
}

#[test]
fn random_failure_is_propagated_without_fallback() {
    assert_eq!(
        generate_with(Options::default(), |_| Err(Error::RandomUnavailable)),
        Err(Error::RandomUnavailable)
    );
}

#[test]
fn rejection_sampling_is_exactly_uniform() {
    for count in [52, 62, 52 + SYMBOLS.len(), 62 + SYMBOLS.len()] {
        let mut frequencies = vec![0; count];
        for byte in 0..=u8::MAX {
            if let Some(index) = uniform_index(byte, count) {
                frequencies[index] += 1;
            }
        }
        assert!(frequencies
            .iter()
            .all(|&frequency| frequency == 256 / count));
    }
}
