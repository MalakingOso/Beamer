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

/// How many points are sampled per placement. Twelve is enough for the spacing
/// to look deliberate; more converges on a lattice, fewer starts to clump.
const CANDIDATES: usize = 12;

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
/// `occupied` holds the top-left corner of every note already on screen. The
/// returned point is the top-left corner for the new one.
///
/// Once the screen is full the candidates stop being far apart and notes begin
/// to overlap. That is the intended degradation: this must never loop forever
/// searching for space that does not exist, and must never push a note
/// off-screen to find it.
pub fn place_next(work_area: Rect, note_size: (u32, u32), occupied: &[(i32, i32)]) -> (i32, i32) {
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

    // Seeded from the note count alone, so a given desktop always lays out the
    // same way — including across restarts, which is what makes the layout
    // reproducible without persisting anything.
    let mut state = (occupied.len() as u32)
        .wrapping_mul(2_654_435_761)
        .wrapping_add(0x9E37_79B9);

    let mut best = (min_x, min_y);
    let mut best_score = -1.0_f64;
    for _ in 0..CANDIDATES {
        let x = min_x + (lcg(&mut state) % span_x) as i32;
        let y = min_y + (lcg(&mut state) % span_y) as i32;
        let score = nearest_distance((x, y), occupied);
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
    const NOTE: (u32, u32) = (320, 260);

    /// Place `n` notes one after another, feeding each result back in as
    /// occupied — the same way the reconciler does when it restores a session.
    fn scatter(n: usize) -> Vec<(i32, i32)> {
        let mut placed: Vec<(i32, i32)> = Vec::new();
        for _ in 0..n {
            let p = place_next(screen(), NOTE, &placed);
            placed.push(p);
        }
        placed
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
            let next = place_next(screen(), NOTE, &placed);
            placed.push(next);
            for p in placed {
                assert_fully_inside(p, screen(), NOTE);
            }
        }
    }

    #[test]
    fn identical_inputs_give_an_identical_result() {
        let occupied = scatter(4);
        let a = place_next(screen(), NOTE, &occupied);
        let b = place_next(screen(), NOTE, &occupied);
        assert_eq!(a, b, "placement must be a pure function of its inputs");
        assert_eq!(
            scatter(6),
            scatter(6),
            "a whole session must lay out the same way twice"
        );
    }

    #[test]
    fn a_handful_of_notes_stay_well_separated() {
        let placed = scatter(5);
        let mut min_sep = f64::MAX;
        for (i, a) in placed.iter().enumerate() {
            for b in &placed[i + 1..] {
                min_sep = min_sep.min(nearest_distance(*a, &[*b]));
            }
        }
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
    fn a_work_area_smaller_than_one_note_does_not_panic() {
        let tiny = Rect { x: 100, y: 100, w: 120, h: 90 };
        let p = place_next(tiny, NOTE, &[]);
        assert!(
            p.0 >= tiny.x
                && p.1 >= tiny.y
                && p.0 <= tiny.x + tiny.w as i32
                && p.1 <= tiny.y + tiny.h as i32,
            "{p:?} escaped the work area {tiny:?} instead of degrading into it"
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
