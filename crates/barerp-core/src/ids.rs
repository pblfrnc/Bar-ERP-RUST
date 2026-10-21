//! Ids no mesmo formato do `@default(cuid())` do Prisma: `c` + timestamp + contador + aleatório, base 36.
//! Mantém a ordenação aproximada por criação e o tamanho de 25 caracteres que o banco já tem.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rand::Rng;

const BASE: u64 = 36;
const BLOCK: usize = 4;

fn base36(mut n: u64, width: usize) -> String {
    let mut buf = vec![b'0'; width];
    for slot in buf.iter_mut().rev() {
        let d = (n % BASE) as u8;
        *slot = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        n /= BASE;
    }
    String::from_utf8(buf).expect("dígitos base 36 são ASCII")
}

/// Novo id único (cuid v1, 25 caracteres).
pub fn cuid() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed) as u64 % BASE.pow(BLOCK as u32);
    let fingerprint = (std::process::id() as u64) % BASE.pow(BLOCK as u32);
    let mut rng = rand::thread_rng();
    let max = BASE.pow(BLOCK as u32);
    format!(
        "c{}{}{}{}{}",
        base36(millis, 8),
        base36(count, BLOCK),
        base36(fingerprint, BLOCK),
        base36(rng.gen_range(0..max), BLOCK),
        base36(rng.gen_range(0..max), BLOCK)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_and_uniqueness() {
        let a = cuid();
        let b = cuid();
        assert_eq!(a.len(), 25);
        assert!(a.starts_with('c'));
        assert!(a.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert_ne!(a, b);
    }
}
