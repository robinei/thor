//! sway's IPC socket (`SWAYSOCK`; the i3 protocol: "i3-ipc", payload length, message type, JSON), with
//! its replies parsed.

use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;

const MAGIC: &[u8; 6] = b"i3-ipc";
/// Event messages have the high bit of their type set.
const EVENT_BIT: u32 = 0x8000_0000;

/// The requests we make.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    RunCommand = 0,
    Subscribe = 2,
    GetOutputs = 3,
    GetTree = 4,
}

/// The events we subscribe to (their type without [`EVENT_BIT`]).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Output = 1,
    Window = 3,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Json(serde_json::Error),
    /// The socket's messages aren't i3-ipc.
    NotIpc,
    /// sway refused a subscription.
    Refused(String),
    /// A launch path with a character sway's command parser can't take inside quotes.
    Unquotable,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Json(e) => write!(f, "{e}"),
            Error::NotIpc => write!(f, "not an i3-ipc message"),
            Error::Refused(what) => write!(f, "sway refused {what}"),
            Error::Unquotable => write!(f, "a path sway can't quote"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

/// How one command of a RUN_COMMAND went.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Outcome {
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Whether every command of a RUN_COMMAND succeeded; the first error if not.
pub fn refused(outcomes: &[Outcome]) -> Option<String> {
    outcomes.iter().find(|o| !o.success).map(|o| o.error.clone().unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Output {
    pub name: String,
    #[serde(default)]
    pub active: bool,
}

/// A node of sway's tree (GET_TREE): the root, outputs, workspaces, containers and windows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Node {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: Option<String>,
    /// A window's process.
    pub pid: Option<u32>,
    /// A Wayland window's app id (X11 windows have a class instead).
    pub app_id: Option<String>,
    pub window_properties: Option<WindowProperties>,
    pub focused: bool,
    /// Marks on it (ours: [`super::commands::DOWN_MARK`] on a window moved to the bottom screen).
    pub marks: Vec<String>,
    pub nodes: Vec<Node>,
    pub floating_nodes: Vec<Node>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WindowProperties {
    pub class: Option<String>,
}

impl Node {
    /// Its children, tiled then floating.
    pub fn children(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().chain(&self.floating_nodes)
    }
}

/// A connection to sway's socket.
pub struct Ipc {
    stream: UnixStream,
}

impl Ipc {
    pub fn connect(socket: &Path) -> io::Result<Ipc> {
        UnixStream::connect(socket).map(|stream| Ipc { stream })
    }

    fn send(&mut self, kind: u32, payload: &str) -> io::Result<()> {
        let len = u32::try_from(payload.len()).map_err(io::Error::other)?;
        let mut message = Vec::with_capacity(14 + payload.len());
        message.extend_from_slice(MAGIC);
        message.extend_from_slice(&len.to_le_bytes());
        message.extend_from_slice(&kind.to_le_bytes());
        message.extend_from_slice(payload.as_bytes());
        self.stream.write_all(&message)
    }

    /// One message: its type and payload.
    fn receive(&mut self) -> Result<(u32, Vec<u8>), Error> {
        let mut header = [0u8; 14];
        self.stream.read_exact(&mut header)?;
        let field = |at: usize| header.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
        let (Some(len), Some(kind), Some(MAGIC)) = (field(6), field(10), header.first_chunk::<6>()) else {
            return Err(Error::NotIpc);
        };
        let mut payload = vec![0; len as usize];
        self.stream.read_exact(&mut payload)?;
        Ok((kind, payload))
    }

    /// A request and its reply (events that come first are skipped).
    fn ask<T: DeserializeOwned>(&mut self, request: Request, payload: &str) -> Result<T, Error> {
        self.send(request as u32, payload)?;
        loop {
            let (kind, body) = self.receive()?;
            if kind == request as u32 {
                return Ok(serde_json::from_slice(&body)?);
            }
        }
    }

    /// Runs commands (`;`-separated); how each went.
    pub fn command(&mut self, text: &str) -> Result<Vec<Outcome>, Error> {
        self.ask(Request::RunCommand, text)
    }

    /// Subscribes to an event; its events then come on this connection.
    pub fn subscribe(&mut self, event: &str) -> Result<(), Error> {
        let outcome: Outcome = self.ask(Request::Subscribe, &serde_json::to_string(&[event])?)?;
        if outcome.success { Ok(()) } else { Err(Error::Refused(format!("the {event} events"))) }
    }

    pub fn outputs(&mut self) -> Result<Vec<Output>, Error> {
        self.ask(Request::GetOutputs, "")
    }

    pub fn tree(&mut self) -> Result<Node, Error> {
        self.ask(Request::GetTree, "")
    }

    /// Waits for the next event; its kind, if one we know.
    pub fn next_event(&mut self) -> Result<Option<EventKind>, Error> {
        let (kind, _) = self.receive()?;
        if kind & EVENT_BIT == 0 {
            log::debug!("sway: unexpected message {kind}");
        }
        Ok(match kind & !EVENT_BIT {
            k if k == EventKind::Output as u32 => Some(EventKind::Output),
            k if k == EventKind::Window as u32 => Some(EventKind::Window),
            _ => None,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let (a, b) = UnixStream::pair().unwrap();
        let (mut a, mut b) = (Ipc { stream: a }, Ipc { stream: b });
        a.send(Request::GetTree as u32, "{}").unwrap();
        assert_eq!(b.receive().unwrap(), (Request::GetTree as u32, b"{}".to_vec()));
        a.send(EVENT_BIT | EventKind::Window as u32, "{}").unwrap();
        assert_eq!(b.next_event().unwrap(), Some(EventKind::Window));
    }

    #[test]
    fn a_reply_after_an_event_is_the_reply() {
        let (a, b) = UnixStream::pair().unwrap();
        let (mut a, mut b) = (Ipc { stream: a }, Ipc { stream: b });
        a.send(EVENT_BIT | EventKind::Window as u32, "{}").unwrap();
        a.send(Request::RunCommand as u32, r#"[{"success": true}, {"success": false, "error": "no such window"}]"#).unwrap();
        let outcomes = b.command("[con_id=1] kill").unwrap();
        assert_eq!(refused(&outcomes).as_deref(), Some("no such window"));
        assert_eq!(refused(&outcomes[..1]), None);
    }

    #[test]
    fn a_tree_parses_with_nulls_and_missing_fields() {
        let tree: Node = serde_json::from_str(
            r#"{"id": 1, "type": "root", "name": "root", "focused": false, "nodes": [{"id": 9, "type": "con", "pid": 200, "app_id": null,
                "name": null, "window_properties": {"class": "steam_app_620", "instance": "x"}, "focused": true}]}"#,
        )
        .unwrap();
        let window = tree.children().next().unwrap();
        assert_eq!((window.pid, window.app_id.as_deref(), window.name.as_deref()), (Some(200), None, None));
        assert_eq!(window.window_properties.as_ref().and_then(|p| p.class.as_deref()), Some("steam_app_620"));
    }
}
