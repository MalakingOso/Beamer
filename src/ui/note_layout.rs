//! Where to put the next sticky note.
//!
//! Beamer places notes itself rather than remembering where they were. On
//! Wayland a client cannot read its own position back (see the module docs on
//! `shell_window`), and the geometry-capture machinery that would be needed to
//! try is both unreliable and racy against window close. Choosing the layout
//! instead of recovering it turns the hardest part of the feature into a pure
//! function — which is why this module has real tests and the alternative
//! could not have had any.
//!
//! **Algorithm: best-candidate sampling.** Sample a handful of points, keep
//! whichever one is furthest from every note already on screen. Repeated, that
//! produces blue-noise-style spacing: it looks hand-scattered, never clumps,
//! and never lands on a visible lattice the way a grid or a spiral does.
//!
//! Randomness comes from a 6-line LCG rather than the `rand` crate. Determinism
//! is a requirement here, not a convenience — the same inputs must give the
//! same layout or none of the tests below mean anything — and seeding a real
//! RNG to get that back would cost a dependency to arrive in the same place.
//!
//! **The seed comes from the caller, and that is the point.** This function
//! stays pure, so the tests can pin a seed and assert on exact coordinates;
//! the caller supplies a fresh one per launch, so the desktop genuinely
//! re-scatters instead of reproducing yesterday's layout. An earlier version
//! derived the seed from `occupied.len()` alone — pure, tested, and silently
//! guaranteeing that the Nth note landed on the same pixel every single
//! launch, which is the opposite of what the feature promises.

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

/// How many points are sampled per placement.
///
/// Raised from the original twelve. Measured over 200 launches, as the share of
/// notes placed at or beyond `MIN_SEPARATION` rather than falling back to
/// best-available:
///
/// | work area | notes | 12 candidates | 48 candidates |
/// |---|---|---|---|
/// | 1920x1040 | 8 | 56.8% | 67.2% |
/// | 1920x1040 | 20 | 22.8% | 26.9% |
/// | 5120x1400 | 12 | 98.3% | **100%** |
/// | 5120x1400 | 20 | 79.2% | **90.8%** |
///
/// Note what that says, because the intuition is backwards: extra samples buy
/// almost nothing on a *wide* desktop, where twelve already succeed nearly
/// every time. They pay off on a *crowded* one — many notes, or a small screen
/// — which is precisely where clumping is worst and most visible.
///
/// The count is raised rather than the algorithm changed because sampling is
/// O(CANDIDATES × notes) of pure arithmetic. It does not converge on a lattice
/// the way an unbounded search would, because `MIN_SEPARATION` stops the loop
/// as soon as a sample is merely *good enough* — on a two-monitor desktop most
/// placements never draw more than a few.
const CANDIDATES: usize = 48;

/// Separation at which a candidate is accepted outright, in logical pixels.
///
/// Comfortably past a default note's diagonal (~412px), so two notes this far
/// apart cannot meaningfully overlap however they are oriented. Taking the
/// first sample that clears the bar — rather than the best of all of them —
/// is what keeps the scatter looking hand-made: always picking the furthest
/// point drives notes into the corners and edges of the work area, which on a
/// wide two-monitor desktop reads as a deliberate border, not a scatter.
const MIN_SEPARATION: f64 = 460.0;

/// Derive this note's seed from the launch seed and its index in the pass.
///
/// Both halves are load-bearing. Without the launch seed every session lays
/// out identically; without the index every note in a single pass would sample
/// the *same* `CANDIDATES` points, so a whole restore would have at most
/// `CANDIDATES` distinct positions to choose between and notes would land
/// exactly on top of one another.
pub fn seed_for(launch_seed: u32, index: usize) -> u32 {
    launch_seed ^ (index as u32).wrapping_mul(2_654_435_761)
}

/// One step of a linear congruential generator (Numerical Recipes constants).
///
/// The low bits of an LCG are famously non-random, so the high half is folded
/// down over the low before returning — otherwise `% span` below would see the
/// worst bits of the word.
fn lcg(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state ^ (*state >> 16)
}

/// Distance from `p` to the closest already-placed note.
///
/// An empty desktop yields `f64::MAX`, which makes every candidate tie on the
/// first note; `place_next` breaks that tie by keeping the first sample, so the
/// result stays deterministic rather than depending on iteration order.
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

/// Where to put the next note, given where the currently-open ones are.
///
/// `occupied` holds the top-left corner of every note already on screen, plus
/// any other point notes should keep clear of — the caller feeds in the main
/// window's corners so notes do not vanish behind it. The returned point is
/// the top-left corner for the new note.
///
/// `seed` makes this a pure function of its arguments: the same seed and the
/// same occupancy always give the same point. The caller varies it per launch
/// and per note; see `seed_for`.
///
/// Once the screen is full the candidates stop being far apart and notes begin
/// to overlap. That is the intended degradation: this must never loop forever
/// searching for space that does not exist, and must never push a note
/// off-screen to find it.
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

    // A work area smaller than one note plus its margins leaves no valid range
    // to sample from. Return the work area's own corner: still on screen, and
    // no inverted range to underflow.
    if max_x < min_x || max_y < min_y {
        return (work_area.x, work_area.y);
    }

    let span_x = (max_x - min_x) as u32 + 1;
    let span_y = (max_y - min_y) as u32 + 1;

    // A zero seed would leave the LCG's first output entirely determined by the
    // additive constant, so the caller's seed is folded in rather than used
    // directly.
    let mut state = seed
        .wrapping_mul(2_654_435_761)
        .wrapping_add(0x9E37_79B9);

    let mut best = (min_x, min_y);
    let mut best_score = -1.0_f64;
    for _ in 0..CANDIDATES {
        let x = min_x + (lcg(&mut state) % span_x) as i32;
        let y = min_y + (lcg(&mut state) % span_y) as i32;
        let score = nearest_distance((x, y), occupied);
        // Good enough beats best available: stop here rather than drawing the
        // rest and drifting towards the extremes of the work area.
        if score >= MIN_SEPARATION {
            return (x, y);
        }
        if score > best_score {
            best_score = score;
            best = (x, y);
        }
    }
    // Nothing cleared the bar. The loop is bounded, so a crowded desktop
    // degrades into overlap here rather than searching forever for space that
    // does not exist.
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

    /// An arbitrary but fixed launch seed. Every assertion about *spacing*
    /// rather than *variety* pins this, so a failure means the algorithm
    /// changed rather than the dice landing differently.
    const SEED: u32 = 0xC0FF_EE01;

    /// Place `n` notes one after another, feeding each result back in as
    /// occupied — the same way the reconciler does when it restores a session,
    /// including deriving each note's seed from the launch seed and its index.
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
        // The defect this pins: seeding from `occupied.len()` made the Nth note
        // land on the same pixel every launch, however many times you restarted.
        // Purity above and variety here are the two halves of the requirement,
        // and either one alone is satisfiable by a wrong implementation.
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
        // Every note in a restore is placed in one pass. If they all shared a
        // seed they would draw the same candidate points and pile up; the index
        // mixing in `seed_for` is what prevents it.
        assert_ne!(seed_for(SEED, 0), seed_for(SEED, 1));
        let placed = scatter(8);
        let unique: std::collections::HashSet<_> = placed.iter().collect();
        assert_eq!(unique.len(), placed.len(), "duplicate positions in {placed:?}");
    }

    #[test]
    fn a_handful_of_notes_stay_well_separated() {
        let placed = scatter(5);
        let min_sep = closest_pair(&placed);
        // Comfortably more than half a note's diagonal (~206px), so no note can
        // substantially cover another. This "well spaced" property is the whole
        // point of the module — a placement that merely stays on screen would
        // pass every other test here.
        assert!(
            min_sep > 250.0,
            "notes clumped: closest pair was {min_sep:.0}px apart in {placed:?}"
        );
    }

    #[test]
    fn a_two_monitor_desktop_holds_every_note_past_the_separation_target() {
        // The case the widening is for. Eight notes is more than a typical
        // session and the area is large enough that every one of them should
        // clear `MIN_SEPARATION` outright rather than falling back to
        // best-available.
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
        // Placing every note on the primary monitor would satisfy every other
        // test in this module while leaving half the desktop empty — which is
        // exactly what the single-monitor `work_area` used to do.
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
        // The main window is fed in as occupied points so notes do not land
        // behind it. It is ordinary occupancy to this function, but it is the
        // reason the caller bothers, so the property is worth pinning.
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
        // Deterministic — fixed seeds, no clock — but statistical in shape, so
        // it measures the thing the constant is actually for rather than
        // pinning coordinates that any tweak would break.
        //
        // Both figures fail at the original CANDIDATES = 12, which measures
        // 56.8% and 79.2% respectively. Without this, nothing in the suite
        // distinguishes the two values: on a wide desktop at ordinary note
        // counts twelve samples already clear the bar almost every time.
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

