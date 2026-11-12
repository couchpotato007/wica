//! Fuzzy name matching for when you half-remember a name: "deer" -> tealdeer,
//! "ripgrap" -> ripgrep, "imagemagic" -> magick.

/// Below this a name isn't shown as a match.
pub const MIN_SCORE: f32 = 0.5;

/// How well `query` matches `name`, from 0 (unrelated) to 1 (identical).
pub fn score(query: &str, name: &str) -> f32 {
    let q = normalize(query);
    let n = normalize(name);
    if q.is_empty() || n.is_empty() {
        return 0.0;
    }
    if q == n {
        return 1.0;
    }
    let (ql, nl) = (q.chars().count(), n.chars().count());
    // How much of the name the query covers: "deer" is half of "tealdeer".
    let coverage = ql as f32 / nl as f32;
    let mut best: f32 = 0.0;

    if n.starts_with(&q) {
        best = best.max(0.85 + 0.1 * coverage);
    } else if ql >= 3 && n.contains(&q) {
        // Not for two letters: "rg" appears inside too many names ("cargo").
        best = best.max(0.6 + 0.3 * coverage);
    }
    // The name is part of the query: "ripgrepper" -> rg is a stretch, ripgrep isn't.
    if nl >= 4 && nl * 2 >= ql && q.contains(&n) {
        best = best.max(0.4 + 0.4 * nl as f32 / ql as f32);
    }
    // Typos in the whole name: "ripgrap", "tealder".
    let d = strsim::damerau_levenshtein(&q, &n);
    if d <= allowed_typos(ql.max(nl)) {
        best = best.max(0.8 - 0.1 * d as f32);
    }
    // Typos in a part of the name: "tealdear" -> tealdeer, "ffprob" -> ffprobe.
    // Not for short queries: one typo in four letters matches half of all names.
    if ql >= 5 && nl > ql {
        let d = best_window_distance(&q, &n);
        if d <= allowed_typos(ql) {
            best = best.max(0.68 - 0.1 * d as f32 + 0.1 * coverage);
        }
    }
    best
}

/// Lowercase and drop separators, so `docker-compose` == `dockercompose`.
fn normalize(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '-' | '_' | '.' | ' ')).flat_map(char::to_lowercase).collect()
}

fn allowed_typos(len: usize) -> usize {
    match len {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

/// Smallest edit distance between `q` and any substring of `n` of similar length.
fn best_window_distance(q: &str, n: &str) -> usize {
    let n: Vec<char> = n.chars().collect();
    let ql = q.chars().count();
    let mut best = usize::MAX;
    for len in ql - 1..=ql + 1 {
        for start in 0..=n.len().saturating_sub(len) {
            let window: String = n[start..(start + len).min(n.len())].iter().collect();
            best = best.min(strsim::damerau_levenshtein(q, &window));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_sensibly() {
        assert_eq!(score("rg", "rg"), 1.0);
        assert!(score("deer", "tealdeer") >= 0.7);
        assert!(score("tealdear", "tealdeer") >= MIN_SCORE);
        assert!(score("deer", "blender") < MIN_SCORE);
        assert!(score("deer", "ledger") < MIN_SCORE);
        assert!(score("imagemagic", "age") < MIN_SCORE);
        assert!(score("imagemagic", "imagemagick") >= 0.7);
        assert!(score("ripgrap", "ripgrep") >= 0.65);
        assert!(score("docker compose", "docker-compose") == 1.0);
        assert!(score("ffm", "ffmpeg") > score("ffm", "ffmpegthumbnailer"));
        assert!(score("deer", "grep") < MIN_SCORE);
        assert!(score("deer", "dexter") < MIN_SCORE);
        assert!(score("rg", "cargo") < MIN_SCORE);
        assert!(score("cat", "bat") < MIN_SCORE);
    }
}
