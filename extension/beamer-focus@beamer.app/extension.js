import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

import { BeamerIndicator } from './indicator.js';

const HELPER_VERSION = 2;

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
        this._indicator = null;
        this._dbus = Gio.DBusExportedObject.wrapJSObject(DBUS_XML, this);
        this._dbus.export(Gio.DBus.session, '/app/beamer/FocusProvider');
    }

    disable() {
        if (this._typeSource) {
            GLib.source_remove(this._typeSource);
            this._typeSource = 0;
        }
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
        this._typeSource = GLib.timeout_add(GLib.PRIORITY_DEFAULT, TICK_MS, () => {
            const end = Math.min(i + CHARS_PER_TICK, cps.length);
            for (; i < end; i++) {
                const keyval = keysymForCodepoint(cps[i]);
                this._notifyKey(device, keyval, true);
                this._notifyKey(device, keyval, false);
            }
            if (i >= cps.length) {
                this._typeSource = 0;
                invocation.return_value(new GLib.Variant('(b)', [true]));
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
}
