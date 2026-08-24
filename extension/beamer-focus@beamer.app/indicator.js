import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import St from 'gi://St';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

// Recording pill in the app's Deploy Purple design language: light surface,
// structural border, hard-offset shadow, at bottom-center of the active
// monitor (the focused window's — where dictated text lands), with a
// purple-gradient waveform while recording and a "Transcribing…" label while
// processing. Added directly to
// uiGroup (layout.js documents this as the supported way to place actors
// above all windows) rather than via addTopChrome: GNOME 50 removed the
// affectsInputRegion chrome param, and an untracked non-reactive actor is
// click-through on every shell version — so the pill can never take focus
// (which would break text injection into the previously focused window).

const BAR_COUNT = 12;
const BAR_WIDTH = 3;
const BAR_MIN_H = 4;
const BAR_MAX_H = 26;
const FRAME_MS = 33; // ~30 fps
const BOTTOM_MARGIN = 32;
const COLOR_FROM = [0x4b, 0x00, 0x82]; // --accent
const COLOR_TO = [0x5c, 0x1a, 0x9e]; // --accent-hover

export class BeamerIndicator {
    constructor() {
        this._pill = new St.BoxLayout({
            style_class: 'beamer-pill',
            visible: false,
            reactive: false,
            track_hover: false,
        });
        this._pill.set_pivot_point(0.5, 1.0);

        this._barsBox = new St.BoxLayout({
            style_class: 'beamer-pill-bars',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._bars = [];
        for (let i = 0; i < BAR_COUNT; i++) {
            const t = i / (BAR_COUNT - 1);
            const rgb = COLOR_FROM.map(
                (c, j) => Math.round(c + (COLOR_TO[j] - c) * t));
            const bar = new St.Widget({
                y_align: Clutter.ActorAlign.CENTER,
                style: `background-color: rgb(${rgb[0]},${rgb[1]},${rgb[2]}); border-radius: 2px;`,
            });
            bar.set_size(BAR_WIDTH, BAR_MIN_H);
            this._bars.push(bar);
            this._barsBox.add_child(bar);
        }

        this._statusLabel = new St.Label({
            text: 'Transcribing…',
            style_class: 'beamer-pill-status',
            y_align: Clutter.ActorAlign.CENTER,
            visible: false,
        });

        this._pill.add_child(this._barsBox);
        this._pill.add_child(this._statusLabel);
        Main.layoutManager.uiGroup.add_child(this._pill);

        this._state = null;
        this._level = 0;
        this._smoothLevel = 0;
        this._phase = 0;
        this._animSource = 0;

        this._monitorsChangedId = Main.layoutManager.connect(
            'monitors-changed', () => this._reposition());
        this._widthChangedId = this._pill.connect(
            'notify::width', () => this._reposition());
    }

    show(state) {
        const wasVisible = this._pill.visible && this._state !== null;
        this._state = state;

        // `show()` never touched style_class before, so the pill was stuck with
        // whatever it was constructed with. Note capture needs a visually
        // distinct ring, so the class is now driven by state.
        this._pill.style_class = state === 'note'
            ? 'beamer-pill beamer-pill-note'
            : 'beamer-pill';

        // Only the slow, indeterminate stage gets a label. 'recording' and
        // 'note' are both live mic capture and show the waveform alone.
        this._statusLabel.visible = state === 'processing';

        if (!this._animSource) {
            this._animSource = GLib.timeout_add(
                GLib.PRIORITY_DEFAULT, FRAME_MS, () => {
                    this._animateBars();
                    return GLib.SOURCE_CONTINUE;
                });
        }

        if (!wasVisible) {
            this._pill.visible = true;
            this._reposition();
            this._pill.opacity = 0;
            this._pill.translation_y = 20;
            this._pill.set_scale(0.92, 0.92);
            this._pill.ease({
                opacity: 255,
                translation_y: 0,
                scale_x: 1,
                scale_y: 1,
                duration: 250,
                mode: Clutter.AnimationMode.EASE_OUT_QUAD,
            });
        }
    }

    setLevel(level) {
        this._level = Math.min(1, Math.max(0, level));
    }

    hide() {
        if (!this._pill.visible) return;
        this._state = null;
        this._pill.ease({
            opacity: 0,
            translation_y: 20,
            scale_x: 0.92,
            scale_y: 0.92,
            duration: 200,
            mode: Clutter.AnimationMode.EASE_IN_QUAD,
            onComplete: () => {
                this._pill.visible = false;
                this._stopAnimation();
            },
        });
    }

    destroy() {
        this._stopAnimation();
        if (this._monitorsChangedId) {
            Main.layoutManager.disconnect(this._monitorsChangedId);
            this._monitorsChangedId = 0;
        }
        if (this._widthChangedId) {
            this._pill.disconnect(this._widthChangedId);
            this._widthChangedId = 0;
        }
        Main.layoutManager.uiGroup.remove_child(this._pill);
        this._pill.destroy();
        this._pill = null;
    }

    _stopAnimation() {
        if (this._animSource) {
            GLib.source_remove(this._animSource);
            this._animSource = 0;
        }
    }

    _animateBars() {
        this._phase += 0.35;
        // Recording follows the live mic level; processing idles at a calm
        // constant sweep. The smoothing keeps bar motion fluid between the
        // ~15 Hz level updates coming over D-Bus. Attack is faster than
        // release so the pill snaps up on voice onset instead of oozing up.
        const target = (this._state === 'recording' || this._state === 'note')
            ? Math.max(0.12, this._level)
            : 0.15;
        const rate = target > this._smoothLevel ? 0.45 : 0.15;
        this._smoothLevel += (target - this._smoothLevel) * rate;
        for (let i = 0; i < BAR_COUNT; i++) {
            // The per-bar sine shimmer is scaled by the level itself, so it
            // reads as "reacting to speech" rather than a decorative sweep
            // that runs the same whether or not you're talking: near-silent
            // bars sit almost still, and the shimmer's full swing only shows
            // up once the level is actually high.
            const shimmer = 0.35 + 0.65 * Math.abs(Math.sin(this._phase + i * 0.55));
            const wave = 1 - this._smoothLevel * (1 - shimmer);
            const h = BAR_MIN_H
                + (BAR_MAX_H - BAR_MIN_H) * this._smoothLevel * wave;
            this._bars[i].set_height(Math.round(Math.min(BAR_MAX_H, h)));
        }
    }

    _reposition() {
        // Follow the focused window's monitor — that's where dictated text
        // lands. currentMonitor (pointer) covers the no-focus case;
        // get_monitor() can return -1 for unmanaged windows, which indexes
        // to undefined and falls through.
        const lm = Main.layoutManager;
        const focusWin = global.display.focus_window;
        const mon = (focusWin && lm.monitors[focusWin.get_monitor()]) ??
            lm.currentMonitor ?? lm.primaryMonitor;
        if (!mon || !this._pill) return;
        this._pill.set_position(
            mon.x + Math.round((mon.width - this._pill.width) / 2),
            mon.y + mon.height - this._pill.height - BOTTOM_MARGIN);
    }
}
