//! Where to put the next sticky note. Beamer places notes rather than
//! remembering where they were (Wayland offers no position read-back).
//! Best-candidate sampling: try a handful of points, keep the furthest from
//! every open note — hand-scattered spacing, no lattice. A tiny inline LCG
//! keeps it deterministic (same inputs, same layout) without a `rand`
//! dependency; the caller supplies the seed per launch (see `seed_for`).

/// A rectangle in the compositor's logical coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Gap kept between a note and the edge of the work area.
const MARGIN: i32 = 24;

/// Samples per placement. 48, not 12: measured over 200 launches, clearance
/// (share at/beyond `MIN_SEPARATION`) rose 56.8%→67.2% (8 notes, 1080p) and
/// 79.2%→90.8% (20 notes, dual 4K) — extra samples pay off on crowded screens.
const CANDIDATES: usize = 48;

/// Separation at which a candidate is accepted outright (logical px). Past a
/// default note's diagonal, so two such notes can't meaningfully overlap.
/// First-past-the-bar wins (not best-of-all) to avoid driving notes into corners.
const MIN_SEPARATION: f64 = 460.0;

/// This note's seed = launch seed mixed with its index. Without the launch part
/// every session lays out identically; without the index every note in one pass
/// draws the same candidates and piles up.
pub fn seed_for(launch_seed: u32, index: usize) -> u32 {
    launch_seed ^ (index as u32).wrapping_mul(2_654_435_761)
}

/// One LCG step (Numerical Recipes). High half folded down — the low bits are
/// the least random, and `% span` below would otherwise see the worst of the word.
fn lcg(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state ^ (*state >> 16)
}

/// Distance from `p` to the closest placed note (`f64::MAX` when empty; the
/// first-sample tiebreak in `place_next` keeps that case deterministic).
fn nearest_distance(p: (i32, i32), occupied: &[(i32, i32)]) -> f64 {
    occupied
        .iter()
        .map(|q| {
            let dx = f64::from(p.0 - q.0);
            let dy = f64::from(p.1 - q.1);
            (dx * dx + dy * dy).sqrt()
        })
        .fold(f64::MAX, f64::min)
}

/// Next note's top-left corner, given open notes' corners in `occupied` (plus
/// any keep-clear points, e.g. the main window). Pure in `seed` (see `seed_for`).
/// Bounded: a full screen degrades into overlap, never an endless search or
/// an off-screen placement.
pub fn place_next(
    work_area: Rect,
    note_size: (u32, u32),
    occupied: &[(i32, i32)],
    seed: u32,
) -> (i32, i32) {
    let (note_w, note_h) = note_size;
    let min_x = work_area.x + MARGIN;
    let min_y = work_area.y + MARGIN;
    let max_x = work_area.x + work_area.w as i32 - note_w as i32 - MARGIN;
    let max_y = work_area.y + work_area.h as i32 - note_h as i32 - MARGIN;

    // No valid range: return the corner itself (on screen, no underflow).
    if max_x < min_x || max_y < min_y {
        return (work_area.x, work_area.y);
    }

    let span_x = (max_x - min_x) as u32 + 1;
    let span_y = (max_y - min_y) as u32 + 1;

    // Fold the seed in so a zero seed doesn't just yield the LCG constant.
    let mut state = seed
        .wrapping_mul(2_654_435_761)
        .wrapping_add(0x9E37_79B9);

    let mut best = (min_x, min_y);
    let mut best_score = -1.0_f64;
    for _ in 0..CANDIDATES {
        let x = min_x + (lcg(&mut state) % span_x) as i32;
        let y = min_y + (lcg(&mut state) % span_y) as i32;
        let score = nearest_distance((x, y), occupied);
        // Good-enough wins outright; otherwise track the best for overlap fallback.
        if score >= MIN_SEPARATION {
            return (x, y);
        }
        if score > best_score {
            best_score = score;
            best = (x, y);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1080p desktop minus the GNOME panel, and the default note size.
    fn screen() -> Rect {
        Rect { x: 0, y: 40, w: 1920, h: 1040 }
    }

    /// Two 4K monitors side by side at scale 1.5 — 5120x1440 logical, which is
    /// the desktop this module was widened for. The seam sits at x = 2560.
    fn two_monitors() -> Rect {
        Rect { x: 0, y: 40, w: 5120, h: 1400 }
    }
    const SEAM: i32 = 2560;

    const NOTE: (u32, u32) = (320, 260);

    /// Fixed seed for spacing assertions (a failure means the algorithm changed).
    const SEED: u32 = 0xC0FF_EE01;

    /// Place `n` notes like the reconciler does, feeding each result back as occupied.
    fn scatter_in(area: Rect, n: usize, launch: u32) -> Vec<(i32, i32)> {
        let mut placed: Vec<(i32, i32)> = Vec::new();
        for i in 0..n {
            let p = place_next(area, NOTE, &placed, seed_for(launch, i));
            placed.push(p);
        }
        placed
    }

    fn scatter(n: usize) -> Vec<(i32, i32)> {
        scatter_in(screen(), n, SEED)
    }

    /// Closest pair in a set of placements.
    fn closest_pair(placed: &[(i32, i32)]) -> f64 {
        let mut min_sep = f64::MAX;
        for (i, a) in placed.iter().enumerate() {
            for b in &placed[i + 1..] {
                min_sep = min_sep.min(nearest_distance(*a, &[*b]));
            }
        }
        min_sep
    }

    fn assert_fully_inside(p: (i32, i32), area: Rect, note: (u32, u32)) {
        assert!(
            p.0 >= area.x && p.1 >= area.y,
            "{p:?} starts before the work area {area:?}"
        );
        assert!(
            p.0 + note.0 as i32 <= area.x + area.w as i32
                && p.1 + note.1 as i32 <= area.y + area.h as i32,
            "{p:?} + {note:?} overflows the work area {area:?}"
        );
    }

    #[test]
    fn placements_are_always_fully_inside_the_work_area() {
        for n in [0usize, 1, 5, 50] {
            let mut placed = scatter(n);
            let next = place_next(screen(), NOTE, &placed, seed_for(SEED, n));
            placed.push(next);
            for p in placed {
                assert_fully_inside(p, screen(), NOTE);
            }
        }
    }

    #[test]
    fn identical_inputs_give_an_identical_result() {
        let occupied = scatter(4);
        let a = place_next(screen(), NOTE, &occupied, SEED);
        let b = place_next(screen(), NOTE, &occupied, SEED);
        assert_eq!(a, b, "placement must be a pure function of its inputs");
        assert_eq!(
            scatter(6),
            scatter(6),
            "one launch seed must lay the same session out the same way twice"
        );
    }

    #[test]
    fn a_different_launch_seed_lays_the_same_notes_out_differently() {
        let a = scatter_in(screen(), 5, 0xC0FF_EE01);
        let b = scatter_in(screen(), 5, 0x1234_5678);
        assert_ne!(a, b, "two launches produced an identical layout: {a:?}");
        assert!(
            a.iter().zip(&b).all(|(p, q)| p != q),
            "some notes reappeared in exactly their old spot: {a:?} vs {b:?}"
        );
    }

    #[test]
    fn one_pass_does_not_reuse_a_single_notes_sample_set() {
        assert_ne!(seed_for(SEED, 0), seed_for(SEED, 1));
        let placed = scatter(8);
        let unique: std::collections::HashSet<_> = placed.iter().collect();
        assert_eq!(unique.len(), placed.len(), "duplicate positions in {placed:?}");
    }

    #[test]
    fn a_handful_of_notes_stay_well_separated() {
        let placed = scatter(5);
        let min_sep = closest_pair(&placed);
        assert!(
            min_sep > 250.0,
            "notes clumped: closest pair was {min_sep:.0}px apart in {placed:?}"
        );
    }

    #[test]
    fn a_two_monitor_desktop_holds_every_note_past_the_separation_target() {
        for launch in [0xC0FF_EE01_u32, 0x1234_5678, 0xDEAD_BEEF, 1, 0] {
            let placed = scatter_in(two_monitors(), 8, launch);
            let min_sep = closest_pair(&placed);
            assert!(
                min_sep >= MIN_SEPARATION,
                "seed {launch:#x}: closest pair {min_sep:.0}px < {MIN_SEPARATION} in {placed:?}"
            );
            for p in &placed {
                assert_fully_inside(*p, two_monitors(), NOTE);
            }
        }
    }

    #[test]
    fn a_two_monitor_scatter_uses_both_monitors() {
        for launch in [0xC0FF_EE01_u32, 0x1234_5678, 0xDEAD_BEEF] {
            let placed = scatter_in(two_monitors(), 6, launch);
            assert!(
                placed.iter().any(|p| p.0 < SEAM),
                "seed {launch:#x}: nothing on the left monitor in {placed:?}"
            );
            assert!(
                placed.iter().any(|p| p.0 >= SEAM),
                "seed {launch:#x}: nothing on the right monitor in {placed:?}"
            );
        }
    }

    #[test]
    fn notes_keep_clear_of_the_points_the_caller_reserves() {
        let main = [(2310, 460), (2810, 460), (2310, 1060), (2810, 1060), (2560, 760)];
        let mut placed: Vec<(i32, i32)> = main.to_vec();
        for i in 0..5 {
            let p = place_next(two_monitors(), NOTE, &placed, seed_for(SEED, i));
            assert!(
                nearest_distance(p, &main) >= MIN_SEPARATION,
                "note {i} landed {:.0}px from the main window at {p:?}",
                nearest_distance(p, &main)
            );
            placed.push(p);
        }
    }

    #[test]
    fn a_work_area_smaller_than_one_note_does_not_panic() {
        let tiny = Rect { x: 100, y: 100, w: 120, h: 90 };
        let p = place_next(tiny, NOTE, &[], SEED);
        assert!(
            p.0 >= tiny.x
                && p.1 >= tiny.y
                && p.0 <= tiny.x + tiny.w as i32
                && p.1 <= tiny.y + tiny.h as i32,
            "{p:?} escaped the work area {tiny:?} instead of degrading into it"
        );
    }

    /// Share of notes placed at or beyond `MIN_SEPARATION` rather than falling
    /// back to best-available, over a fixed set of launch seeds.
    fn clearance_rate(area: Rect, notes: usize, launches: u32) -> f64 {
        let (mut cleared, mut total) = (0u32, 0u32);
        for launch in 0..launches {
            let mut placed: Vec<(i32, i32)> = Vec::new();
            for i in 0..notes {
                let seed = seed_for(launch.wrapping_mul(0x9E37_79B9), i);
                let p = place_next(area, NOTE, &placed, seed);
                total += 1;
                if nearest_distance(p, &placed) >= MIN_SEPARATION {
                    cleared += 1;
                }
                placed.push(p);
            }
        }
        f64::from(cleared) / f64::from(total)
    }

    #[test]
    fn the_candidate_count_is_load_bearing_on_a_crowded_desktop() {
        // Statistical but deterministic (fixed seeds). Both figures fail at CANDIDATES = 12.
        let crowded = clearance_rate(screen(), 8, 200);
        assert!(
            crowded > 0.62,
            "only {:.1}% of notes cleared {MIN_SEPARATION}px on one 1080p screen",
            crowded * 100.0
        );
        let wide = clearance_rate(two_monitors(), 20, 200);
        assert!(
            wide > 0.85,
            "only {:.1}% of 20 notes cleared {MIN_SEPARATION}px across two monitors",
            wide * 100.0
        );
    }

    #[test]
    fn placing_a_hundred_notes_terminates() {
        let placed = scatter(100);
        assert_eq!(placed.len(), 100);
        for p in placed {
            assert_fully_inside(p, screen(), NOTE);
        }
    }
}

