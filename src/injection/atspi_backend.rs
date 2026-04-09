#![cfg(not(target_os = "windows"))]

use super::{InjectionBackend, InjectionResult};
use anyhow::{bail, Context, Result};
use zbus::blocking::{connection, Connection};
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::Address;

pub struct AtspiBackend;

// AT-SPI state bit positions
const STATE_FOCUSED: u32 = 12;
const STATE_SHOWING: u32 = 14;

impl InjectionBackend for AtspiBackend {
    fn name(&self) -> &'static str {
        "atspi"
    }

    fn display_name(&self) -> &'static str {
        "AT-SPI (accessibility direct insert)"
    }

    fn available(&self) -> Result<(), String> {
        get_a11y_bus()
            .map(|_| ())
            .map_err(|e| format!("AT-SPI bus not available: {}", e))
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        let a11y = get_a11y_bus().context("Failed to connect to AT-SPI bus")?;

        let (bus_name, path) = find_focused_editable(&a11y)
            .context("No focused editable text element found via AT-SPI")?;

        insert_text_at_caret(&a11y, &bus_name, &path, text)
            .context("AT-SPI InsertText failed")?;

        Ok(InjectionResult {
            method: "AT-SPI".into(),
            target_info: "via accessibility EditableText.InsertText".into(),
        })
    }
}

/// Connect to the AT-SPI accessibility bus (separate from session bus).
fn get_a11y_bus() -> Result<Connection> {
    let session = Connection::session().context("Failed to connect to session bus")?;

    // Enable AT-SPI if not already enabled (required for apps to register)
    let _ = session.call_method(
        Some("org.a11y.Bus"),
        "/org/a11y/bus",
        Some("org.freedesktop.DBus.Properties"),
        "Set",
        &(
            "org.a11y.Status",
            "IsEnabled",
            Value::from(true),
        ),
    );

    // Get the AT-SPI bus address
    let reply = session
        .call_method(
            Some("org.a11y.Bus"),
            "/org/a11y/bus",
            Some("org.a11y.Bus"),
            "GetAddress",
            &(),
        )
        .context("org.a11y.Bus.GetAddress failed — is at-spi2-core installed?")?;

    let addr: String = reply.body().deserialize().context("Bad GetAddress reply")?;
    tracing::debug!("AT-SPI bus address: {}", addr);

    let address: Address = addr.parse().context("Invalid AT-SPI bus address")?;
    let a11y = connection::Builder::address(address)
        .context("Failed to create AT-SPI connection builder")?
        .build()
        .context("Failed to connect to AT-SPI bus")?;

    Ok(a11y)
}

/// Find the currently focused element that supports EditableText.
/// Walks the accessible tree of all registered applications.
fn find_focused_editable(conn: &Connection) -> Result<(String, String)> {
    // Get registered applications from the registry root
    let reply = conn.call_method(
        Some("org.a11y.atspi.Registry"),
        "/org/a11y/atspi/accessible/root",
        Some("org.a11y.atspi.Accessible"),
        "GetChildren",
        &(),
    )?;

    let children: Vec<(String, OwnedValue)> = reply.body().deserialize()?;
    tracing::debug!("AT-SPI registry has {} registered apps", children.len());

    for (bus_name, _path_val) in &children {
        let root_path = "/org/a11y/atspi/accessible/root";
        match search_focused_editable(conn, bus_name, root_path, 0) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => continue,
            Err(e) => {
                tracing::debug!("Error searching {}: {}", bus_name, e);
                continue;
            }
        }
    }

    bail!("No focused editable text element found in any application")
}

/// Recursively search an app's accessible tree for a focused EditableText element.
fn search_focused_editable(
    conn: &Connection,
    bus_name: &str,
    path: &str,
    depth: u32,
) -> Result<Option<(String, String)>> {
    if depth > 30 {
        return Ok(None);
    }

    // Get the element's state
    let states = get_states(conn, bus_name, path)?;
    let is_focused = has_state(&states, STATE_FOCUSED);

    if is_focused {
        // Check if this element supports EditableText
        let interfaces = get_interfaces(conn, bus_name, path)?;
        if interfaces.iter().any(|i| i == "org.a11y.atspi.EditableText") {
            tracing::debug!(
                "Found focused EditableText: bus={} path={}",
                bus_name,
                path
            );
            return Ok(Some((bus_name.to_string(), path.to_string())));
        }
    }

    // Only recurse into elements that are showing (optimization: skip hidden subtrees)
    if depth > 0 && !has_state(&states, STATE_SHOWING) && !is_focused {
        return Ok(None);
    }

    // Get children and recurse
    let children = get_children(conn, bus_name, path)?;
    for (child_bus, child_path) in children {
        match search_focused_editable(conn, &child_bus, &child_path, depth + 1) {
            Ok(Some(result)) => return Ok(Some(result)),
            Ok(None) => continue,
            Err(_) => continue,
        }
    }

    Ok(None)
}

/// Insert text at the current caret position via EditableText.InsertText.
fn insert_text_at_caret(conn: &Connection, bus_name: &str, path: &str, text: &str) -> Result<()> {
    // Get caret position from Text interface
    let caret = get_caret_offset(conn, bus_name, path).unwrap_or(0);
    tracing::debug!("Caret offset: {}", caret);

    let len = text.len() as i32;
    let reply = conn.call_method(
        Some(bus_name),
        ObjectPath::try_from(path)?,
        Some("org.a11y.atspi.EditableText"),
        "InsertText",
        &(caret, text, len),
    )?;

    let success: bool = reply.body().deserialize().unwrap_or(true);
    if !success {
        bail!("EditableText.InsertText returned false");
    }

    Ok(())
}

fn get_caret_offset(conn: &Connection, bus_name: &str, path: &str) -> Result<i32> {
    let reply = conn.call_method(
        Some(bus_name),
        ObjectPath::try_from(path)?,
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &("org.a11y.atspi.Text", "CaretOffset"),
    )?;
    let val: OwnedValue = reply.body().deserialize()?;
    let offset: i32 = val.try_into()?;
    Ok(offset)
}

fn get_states(conn: &Connection, bus_name: &str, path: &str) -> Result<Vec<u32>> {
    let reply = conn.call_method(
        Some(bus_name),
        ObjectPath::try_from(path)?,
        Some("org.a11y.atspi.Accessible"),
        "GetState",
        &(),
    )?;
    let states: Vec<u32> = reply.body().deserialize()?;
    Ok(states)
}

fn get_interfaces(conn: &Connection, bus_name: &str, path: &str) -> Result<Vec<String>> {
    let reply = conn.call_method(
        Some(bus_name),
        ObjectPath::try_from(path)?,
        Some("org.a11y.atspi.Accessible"),
        "GetInterfaces",
        &(),
    )?;
    let interfaces: Vec<String> = reply.body().deserialize()?;
    Ok(interfaces)
}

fn get_children(conn: &Connection, bus_name: &str, path: &str) -> Result<Vec<(String, String)>> {
    let reply = conn.call_method(
        Some(bus_name),
        ObjectPath::try_from(path)?,
        Some("org.a11y.atspi.Accessible"),
        "GetChildren",
        &(),
    )?;
    // Children are returned as array of (bus_name, object_path) structs
    let raw: Vec<(String, OwnedValue)> = reply.body().deserialize()?;
    let children: Vec<(String, String)> = raw
        .into_iter()
        .filter_map(|(bus, path_val): (String, OwnedValue)| {
            let path_str: String = path_val.try_into().ok()?;
            Some((bus, path_str))
        })
        .collect();
    Ok(children)
}

fn has_state(states: &[u32], bit: u32) -> bool {
    let word = (bit / 32) as usize;
    let bit_pos = bit % 32;
    states.get(word).map_or(false, |w| (w >> bit_pos) & 1 == 1)
}
