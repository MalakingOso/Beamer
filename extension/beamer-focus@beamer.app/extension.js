import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

import { BeamerIndicator } from './indicator.js';

// v5 added PlaceWindow/GetWindowFrame (sticky note placement) and the 'note'
// pill state. The capability floor Beamer requires is still v2 (see
// REQUIRED_VERSION in src/injection/gnome.rs) — every caller of a v5-only
// method degrades to a silent no-op against an older helper.
//
// ⚠️ This number is not only the D-Bus contract version, it is the *deploy
// trigger*. `status()` in src/install/gnome_extension.rs compares what this
// returns over D-Bus against metadata.json's "version"; while they match,
// Beamer reports Enabled and never re-copies the files, so an edit to any
// .js file here silently never reaches ~/.local/share/gnome-shell/. Bump
// BOTH this and metadata.json on every change to this directory, even a
// pure behaviour tweak that adds no method. v6 is exactly that: the pill
// waveform change in 7f83eeb, which sat undeployed because v5 shipped it
// without a bump.
const HELPER_VERSION = 6;

// Typing pace: batches keep long transcripts fast (~500 chars/s) while giving
// slow event loops (Electron apps) time to drain between batches.
const CHARS_PER_TICK = 8;
const TICK_MS = 16;

const DBUS_XML = `
<node>
  <interface name="app.beamer.FocusProvider">
    <method name="GetFocusedAppId">
      <arg type="s" direction="out" name="app_id"/>
    </method>
    <method name="GetVersion">
      <arg type="u" direction="out" name="version"/>
    </method>
    <method name="TypeText">
      <arg type="s" direction="in" name="text"/>
      <arg type="b" direction="out" name="ok"/>
    </method>
    <method name="SendPasteChord">
      <arg type="b" direction="in" name="use_shift"/>
      <arg type="b" direction="out" name="ok"/>
    </method>
    <method name="ShowIndicator">
      <arg type="s" direction="in" name="state"/>
    </method>
    <method name="UpdateLevel">
      <arg type="d" direction="in" name="level"/>
    </method>
    <method name="HideIndicator"/>
    <method name="PlaceWindow">
      <arg type="s" direction="in" name="title"/>
      <arg type="i" direction="in" name="x"/>
      <arg type="i" direction="in" name="y"/>
      <arg type="b" direction="in" name="all_workspaces"/>
      <arg type="b" direction="out" name="ok"/>
    </method>
    <method name="GetWindowFrame">
      <arg type="s" direction="in" name="title"/>
      <arg type="b" direction="out" name="found"/>
      <arg type="i" direction="out" name="x"/>
      <arg type="i" direction="out" name="y"/>
      <arg type="u" direction="out" name="width"/>
      <arg type="u" direction="out" name="height"/>
    </method>
  </interface>
</node>`;

/// Keysym for a Unicode codepoint: Latin-1 codepoints are their own keysym,
/// everything else uses the X11 Unicode offset (0x01000000 | cp). Mutter
/// remaps the keymap on demand for keysyms it doesn't currently have, so this
/// types arbitrary text regardless of the user's layout.
function keysymForCodepoint(cp) {
    return cp < 0x100 ? cp : cp | 0x01000000;
}

export default class BeamerFocusExtension extends Extension {
    enable() {
        this._virtualDevice = null;
        this._typeSource = 0;
        this._typeInvocation = null;
        this._indicator = null;
        this._dbus = Gio.DBusExportedObject.wrapJSObject(DBUS_XML, this);
        this._dbus.export(Gio.DBus.session, '/app/beamer/FocusProvider');
    }

    disable() {
        if (this._typeSource) {
            GLib.source_remove(this._typeSource);
            this._typeSource = 0;
        }
        // A TypeText in flight owes its caller a reply. Cancelling the timeout
        // above without answering left Beamer blocked until its own client-side
        // timeout fired (seconds, scaled to text length). Answer `false` so it
        // falls through to the next injection backend immediately.
        this._finishTyping(false);
        if (this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
        }
        this._virtualDevice = null;
        if (this._dbus) {
            this._dbus.unexport();
            this._dbus = null;
        }
    }

    /// Reply to the pending TypeText invocation, if any, exactly once.
    _finishTyping(ok) {
        const invocation = this._typeInvocation;
        this._typeInvocation = null;
        if (invocation)
            invocation.return_value(new GLib.Variant('(b)', [ok]));
    }

    _ensureVirtualDevice() {
        if (!this._virtualDevice) {
            const seat = Clutter.get_default_backend().get_default_seat();
            this._virtualDevice = seat.create_virtual_device(
                Clutter.InputDeviceType.KEYBOARD_DEVICE);
        }
        return this._virtualDevice;
    }

    _notifyKey(device, keyval, pressed) {
        device.notify_keyval(
            Clutter.get_current_event_time() * 1000,
            keyval,
            pressed ? Clutter.KeyState.PRESSED : Clutter.KeyState.RELEASED);
    }

    // ── D-Bus methods ────────────────────────────────────────────────────────

    GetFocusedAppId() {
        const win = global.display.focus_window;
        if (!win) return '';
        const gtkId = win.get_gtk_application_id?.();
        if (gtkId && gtkId.length > 0) return gtkId;
        const wmClass = win.get_wm_class?.();
        return wmClass ? wmClass.toLowerCase() : '';
    }

    GetVersion() {
        return HELPER_VERSION;
    }

    // Async-return variant (wrapJSObject convention): the reply is sent when
    // the last batch has been typed, so the caller knows when typing finished.
    TypeTextAsync(params, invocation) {
        const [text] = params;
        if (this._typeSource) {
            // A previous TypeText is still running — refuse rather than
            // interleave two transcripts.
            invocation.return_value(new GLib.Variant('(b)', [false]));
            return;
        }
        // Control characters are sanitized out by Beamer; filter defensively
        // so a stray \n can never press Enter in the focused app.
        const cps = [...text]
            .map(c => c.codePointAt(0))
            .filter(cp => cp >= 0x20);
        if (cps.length === 0) {
            invocation.return_value(new GLib.Variant('(b)', [true]));
            return;
        }
        const device = this._ensureVirtualDevice();
        let i = 0;
        this._typeInvocation = invocation;
        this._typeSource = GLib.timeout_add(GLib.PRIORITY_DEFAULT, TICK_MS, () => {
            const end = Math.min(i + CHARS_PER_TICK, cps.length);
            for (; i < end; i++) {
                const keyval = keysymForCodepoint(cps[i]);
                this._notifyKey(device, keyval, true);
                this._notifyKey(device, keyval, false);
            }
            if (i >= cps.length) {
                this._typeSource = 0;
                this._finishTyping(true);
                return GLib.SOURCE_REMOVE;
            }
            return GLib.SOURCE_CONTINUE;
        });
    }

    SendPasteChord(useShift) {
        const device = this._ensureVirtualDevice();
        const seq = useShift
            ? [[Clutter.KEY_Control_L, true], [Clutter.KEY_Shift_L, true],
               [Clutter.KEY_v, true], [Clutter.KEY_v, false],
               [Clutter.KEY_Shift_L, false], [Clutter.KEY_Control_L, false]]
            : [[Clutter.KEY_Control_L, true],
               [Clutter.KEY_v, true], [Clutter.KEY_v, false],
               [Clutter.KEY_Control_L, false]];
        for (const [keyval, pressed] of seq)
            this._notifyKey(device, keyval, pressed);
        return true;
    }

    ShowIndicator(state) {
        if (!this._indicator)
            this._indicator = new BeamerIndicator();
        this._indicator.show(state);
    }

    UpdateLevel(level) {
        this._indicator?.setLevel(level);
    }

    HideIndicator() {
        this._indicator?.hide();
    }

    // ── Window placement ─────────────────────────────────────────────────────
    //
    // Code running inside GNOME Shell is not a Wayland client, so it may do
    // what no client can: read and set a window's absolute position. Beamer
    // matches its sticky notes by exact title (`Beamer Note <id>`, see
    // `window_title` in src/ui/sticky.rs) — the only handle a client and the
    // shell reliably share.

    /// Find a note window by its exact title, preferring one that actually
    /// belongs to Beamer.
    ///
    /// Title is the only handle a Wayland client and the shell reliably share,
    /// but titles are not owned: any window may call itself `Beamer Note <id>`
    /// and be moved or pinned across workspaces in the real note's place. The
    /// app id narrows that to windows Beamer plausibly owns. It is a preference
    /// rather than a requirement because a hard filter that guessed the app id
    /// wrong would break placement silently, and diagnosing it costs a full
    /// GNOME log out — so a title-only match is still honoured, and logged.
    _findWindowByTitle(title) {
        let fallback = null;
        for (const actor of global.get_window_actors()) {
            const win = actor.meta_window;
            if (!win || win.get_title() !== title)
                continue;
            const id = (win.get_gtk_application_id?.() ||
                        win.get_wm_class?.() || '').toLowerCase();
            if (id.includes('beamer'))
                return win;
            fallback = fallback ?? win;
        }
        if (fallback)
            log(`beamer: "${title}" matched a window that is not Beamer's`);
        return fallback;
    }

    /// Move a window and optionally pin it to every workspace.
    ///
    /// Negative coordinates mean "don't move" — pass (-1, -1) to change only
    /// the sticky state of a window already where it should be.
    PlaceWindow(title, x, y, allWorkspaces) {
        const win = this._findWindowByTitle(title);
        if (!win)
            return false;
        if (x >= 0 || y >= 0)
            win.move_frame(true, x, y);
        // Both directions, so flipping the config toggle actually takes effect
        // rather than only ever being able to add stickiness.
        if (allWorkspaces)
            win.stick();
        else
            win.unstick();
        return true;
    }

    /// Read a window's frame rect back in stage coordinates.
    ///
    /// Nothing in Beamer calls this today: notes are auto-placed and their
    /// positions are deliberately not remembered. It ships anyway because
    /// every extension change costs a full GNOME log out, and this is the only
    /// way to verify that a `PlaceWindow` actually landed where it was asked to.
    GetWindowFrame(title) {
        const win = this._findWindowByTitle(title);
        if (!win)
            return [false, 0, 0, 0, 0];
        const r = win.get_frame_rect();
        return [true, r.x, r.y, r.width, r.height];
    }
}
