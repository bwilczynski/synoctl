//! Running blocking network calls a few at a time.

/// Calls at a time.
const PARALLEL: usize = 8;

/// `f` over `items`, at most `PARALLEL` at once, results in `items` order.
/// A panic in `f` is re-raised here.
pub(crate) fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    items
        .chunks(PARALLEL)
        .flat_map(|chunk| {
            std::thread::scope(|s| {
                let handles: Vec<_> = chunk.iter().map(|item| s.spawn(|| f(item))).collect();
                handles
                    .into_iter()
                    .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
                    .collect::<Vec<_>>()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn keeps_order_across_chunks() {
        let items: Vec<usize> = (0..20).collect();
        assert_eq!(
            super::map(&items, |i| i * 2),
            (0..20).map(|i| i * 2).collect::<Vec<_>>()
        );
    }
}
