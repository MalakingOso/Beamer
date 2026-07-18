import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import St from 'gi://St';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

// VibeTyper-style recording pill: dark glass capsule at bottom-center of the
// primary monitor with a purple-gradient waveform, an elapsed timer while
// recording, and a "Transcribing…" label while processing. Added directly to
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
const COLOR_FROM = [0x4b, 0x00, 0x82]; // Beamer deploy purple
const COLOR_TO = [0xa5, 0x61, 0xec]; // bright accent

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

        this._timerLabel = new St.Label({
            text: '0:00',
            style_class: 'beamer-pill-timer',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._statusLabel = new St.Label({
            text: 'Transcribing…',
            style_class: 'beamer-pill-status',
            y_align: Clutter.ActorAlign.CENTER,
            visible: false,
        });

        this._pill.add_child(this._barsBox);
        this._pill.add_child(this._timerLabel);
        this._pill.add_child(this._statusLabel);
        Main.layoutManager.uiGroup.add_child(this._pill);

        this._state = null;
        this._level = 0;
        this._smoothLevel = 0;
        this._phase = 0;
        this._seconds = 0;
        this._animSource = 0;
        this._timerSource = 0;

        this._monitorsChangedId = Main.layoutManager.connect(
            'monitors-changed', () => this._reposition());
        this._widthChangedId = this._pill.connect(
            'notify::width', () => this._reposition());
    }

    show(state) {
        const wasVisible = this._pill.visible && this._state !== null;
        this._state = state;

        if (state === 'recording') {
            this._seconds = 0;
            this._timerLabel.text = '0:00';
            this._timerLabel.visible = true;
            this._statusLabel.visible = false;
            if (!this._timerSource) {
                this._timerSource = GLib.timeout_add_seconds(
                    GLib.PRIORITY_DEFAULT, 1, () => {
                        this._seconds++;
                        const m = Math.floor(this._seconds / 60);
                        const s = `${this._seconds % 60}`.padStart(2, '0');
                        this._timerLabel.text = `${m}:${s}`;
                        return GLib.SOURCE_CONTINUE;
                    });
            }
        } else {
            this._stopTimer();
            this._timerLabel.visible = false;
            this._statusLabel.visible = true;
        }

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
        this._stopTimer();
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
        this._stopTimer();
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

    _stopTimer() {
        if (this._timerSource) {
            GLib.source_remove(this._timerSource);
            this._timerSource = 0;
        }
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
        // ~15 Hz level updates coming over D-Bus.
        const target = this._state === 'recording'
            ? Math.max(0.12, this._level)
            : 0.15;
        this._smoothLevel += (target - this._smoothLevel) * 0.3;
        for (let i = 0; i < BAR_COUNT; i++) {
            const wave = 0.35 + 0.65 * Math.abs(Math.sin(this._phase + i * 0.55));
            const h = BAR_MIN_H
                + (BAR_MAX_H - BAR_MIN_H) * this._smoothLevel * wave;
            this._bars[i].set_height(Math.round(Math.min(BAR_MAX_H, h)));
        }
    }

    _reposition() {
        const mon = Main.layoutManager.primaryMonitor;
        if (!mon || !this._pill) return;
        this._pill.set_position(
            mon.x + Math.round((mon.width - this._pill.width) / 2),
            mon.y + mon.height - this._pill.height - BOTTOM_MARGIN);
    }
}
