//! Idfon CLI: thin client over the idfond Unix-socket IPC. Argument parsing
//! is clap derive with noun-verb subcommands (`idfon peer add`,
//! `idfon live stop`, ...) that map onto daemon methods (`peer.add`,
//! `media.live.stop`, ...). One or more request/response exchanges per
//! connection, matching the daemon's `read_frame`/`write_frame` loop
//! (`crates/idfond/src/main.rs`).
//!
//! Output contracts the scripts rely on:
//! - `--json` prints the full `Response` envelope; default is human output.
//! - `put`/`stream`/`send-data` print a bare ticket; `get`/`listen` stream
//!   bytes; bare `wait` prints one JSON line per event.
//! - `--stdin-json` (hidden) replaces the request params wholesale from
//!   stdin, for RPC pass-through scripting.

use std::{
    env, io,
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    sync::OnceLock,
    time::Duration,
};

use clap::{Parser, Subcommand};

mod ux;
use idfon_client::{Client, ClientError};
use idfon_protocol::{
    encode_json,
    vcf::{contact_ticket_vcard, vcard_contact_tickets},
    rpc::{self, Method},
    ContactTicket, DeliveryMode, DeliveryPolicy, ErrorCode, Peer, Request, Response,
    ResponseBody,
};
use serde_json::{json, Value};

/// The active profile (`--profile` / `IDFON_PROFILE`), set once in `run`.
static PROFILE: OnceLock<Option<String>> = OnceLock::new();
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(5);
/// Auto-started daemons exit after this long with no connected clients.
const DAEMON_IDLE_EXIT_SECS: &str = "600";
/// `idfon --keep-alive`: spawned daemon never idles out.
static KEEP_ALIVE: OnceLock<bool> = OnceLock::new();
/// Relay flags forwarded to an auto-started daemon (per-run parameter,
/// docs/relay-config.md).
static RELAY_ARGS: OnceLock<Vec<String>> = OnceLock::new();

/// Locates idfond: next to the idfon binary first (cargo builds them
/// together), then PATH.
fn find_daemon_binary() -> Option<PathBuf> {
    let sibling = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("idfond")));
    match sibling {
        Some(path) if path.is_file() => Some(path),
        _ => env::var_os("PATH").and_then(|paths| {
            env::split_paths(&paths)
                .map(|dir| dir.join("idfond"))
                .find(|path| path.is_file())
        }),
    }
}

#[derive(Parser)]
#[command(
    name = "idfon",
    version,
    about = "idfon: message, share files and stream audio with your contacts, peer to peer",
    after_help = "Examples:
  idfon peer add carol.vcf               add a contact from a card, link or ticket
  idfon send bob \"hi\" --wait             message bob and print his reply
  idfon inbox --from bob                 messages you received
  idfon send bob --file notes.txt        send a file;  idfon recv -o copy.txt  to receive one
  idfon put notes.txt                    store a file, print a download ticket
  idfon get TICKET -o copy.txt           download a ticket
  idfon live publish song.wav --loop     broadcast audio, print a live ticket
  idfon call bob --file song.wav         1:1 call;  idfon answer  to take one
  idfon access ticket bob --expires 30d  let bob message you
  idfon rpc peer.list                    any daemon method, raw JSON

Peers are named by name, alias or id everywhere. Add --json for machine output."
)]
struct Cli {
    /// Profile: a separate daemon, socket and data dir (default: IDFON_PROFILE)
    #[arg(long, global = true, value_name = "NAME")]
    profile: Option<String>,
    /// Daemon socket path (overrides the profile's)
    #[arg(long, global = true, value_name = "PATH")]
    socket: Option<String>,
    /// Daemon identity to act as
    #[arg(long, global = true, value_name = "ID")]
    identity: Option<String>,
    /// Machine output: print the full Response envelope
    #[arg(long, global = true)]
    json: bool,
    /// Auto-started daemon: disable idle shutdown (stays running until `idfon shutdown`)
    #[arg(long, global = true, hide = true)]
    keep_alive: bool,
    /// Auto-started daemon: relay URLs (per-run parameter; docs/relay-config.md)
    #[arg(long, global = true, value_name = "URLS", hide = true)]
    relay_urls: Option<String>,
    /// Auto-started daemon: relay shared bearer token
    #[arg(long, global = true, value_name = "TOKEN", hide = true)]
    relay_token: Option<String>,
    /// Auto-started daemon: drop IP transports (relay-only)
    #[arg(long, global = true, hide = true)]
    relay_only: bool,
    /// Replace request params with a JSON document read from stdin
    #[arg(long, global = true, hide = true)]
    stdin_json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Send a message (`send bob "hi"`) or a file (`send bob --file F`) to a peer
    Send(SendArgs),
    /// Show messages you received (`--from PEER`, `--follow`)
    Inbox(InboxArgs),
    /// Save a file a peer sent you (waits for one if none arrived recently)
    Recv(RecvArgs),
    /// Manage contacts (`peer add` takes a .vcf card, idfon://add link or ticket)
    #[command(subcommand)]
    Peer(PeerCmd),
    /// Rooms: group messaging with a member list
    #[command(subcommand)]
    Room(RoomCmd),
    /// Store a file; prints a download ticket anyone can use
    Put(PutArgs),
    /// Download a ticket (blob tickets: bytes; live tickets: record audio)
    Get(GetArgs),
    /// Live audio broadcasts: publish, list, stop, watch
    #[command(subcommand)]
    Live(LiveCmd),
    /// Call a peer 1:1, streaming FILE (blocks until they hang up)
    Call(CallArgs),
    /// Answer the next incoming 1:1 call and record it
    Answer(AnswerArgs),
    /// Who may message you: check, grant, or hand out tickets
    #[command(subcommand)]
    Access(AccessCmd),
    /// Show daemon status
    Status,
    /// Verify local identity and peer channel endpoint consistency
    Doctor,
    /// Stop the background daemon
    Shutdown,
    /// Manage identities
    #[command(subcommand)]
    Identity(IdentityCmd),
    /// vCard exchange: export a card carrying the idfon ticket, import .vcf files
    #[command(subcommand)]
    Contact(ContactCmd),
    /// Fetch daemon events (follow with --follow)
    Events(EventsArgs),
    /// Block until a matching event arrives (--json prints the Response envelope,
    /// like events without --follow)
    Wait(WaitArgs),
    /// Track async operations
    #[command(subcommand)]
    Operation(OperationCmd),
    /// MCP transport relay (user side): open a local socket to a peer's MCP server
    #[command(subcommand)]
    Mcp(McpCmd),
    /// Publish FILE to the edge's durable files store; prints its URL
    /// (the free tier is `idfon put`, a connected-only BlobTicket)
    Publish(PublishArgs),
    /// Loopback HTTP gateway for `idfon://` resources
    #[command(subcommand)]
    Gateway(GatewayCmd),
    /// Serve this identity's stored resources over HTTP/3 to granted peers
    #[command(subcommand)]
    Provider(ProviderCmd),
    /// Fetch an `idfon://<account>/<path>` resource through the gateway
    Fetch(FetchArgs),
    /// Print the public edge URL for a resource (`<ref>/<path>`)
    #[command(subcommand)]
    Web(WebCmd),
    /// Where idfon keeps things, and its settings (`config show`)
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Call any daemon method with raw JSON params (`rpc peer.list`)
    Rpc {
        /// Method name, e.g. peer.list (see docs/protocol.md)
        method: String,
        /// JSON params; `-` reads stdin; default `{}`
        params: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProviderCmd {
    /// Serve a user-visible directory at `GET /fs/<path>` to granted peers
    Start {
        /// Absolute path of the directory to serve (live, in place)
        #[arg(long, value_name = "DIR")]
        root: String,
    },
    /// Stop this identity's H3 resource provider
    Stop,
}

#[derive(Subcommand)]
enum GatewayCmd {
    /// Bind this identity's loopback gateway; prints its URL and token
    Start,
    /// Stop this identity's loopback gateway
    Stop,
}

#[derive(clap::Args)]
struct FetchArgs {
    /// `idfon://<account>/<path>` resource URL
    url: String,
    /// Write the body to FILE instead of stdout
    #[arg(long)]
    out: Option<String>,
    /// Routing: `auto` (direct, then edge), `direct`, or `edge`. `edge` needs
    /// `IDFON_EDGE_URL` (and token/ticket) on the daemon.
    #[arg(long, default_value = "auto", value_parser = ["auto", "direct", "edge"])]
    prefer: String,
}

#[derive(Subcommand)]
enum WebCmd {
    /// Print the public edge URL for `<ref>/<path>` (needs IDFON_EDGE_URL on the daemon)
    Open {
        /// `<ref>/<path>` (path optional)
        target: String,
    },
}

#[derive(Subcommand)]
enum IdentityCmd {
    /// List identities
    List,
    /// Switch the active identity
    Use { name: String },
    /// Create an identity
    Create { name: String },
    /// Delete an identity
    Delete { name: String },
}

#[derive(Subcommand)]
enum PeerCmd {
    /// List peers
    List,
    /// Add a contact: a .vcf card, an idfon://add/ link, an MCP ticket, or REF with --endpoint-id
    Add {
        /// carol.vcf | idfon://add/... | MCP ticket JSON | a peer id with --endpoint-id/--endpoint-addr
        #[arg(value_name = "INPUT")]
        peer_ref: Option<String>,
        /// Stable account/virtual ID; defaults to REF for legacy direct peers
        #[arg(long = "account-id")]
        account_id: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long = "endpoint-id")]
        endpoint_id: Option<String>,
        #[arg(long = "endpoint-addr")]
        endpoint_addr: Option<String>,
        /// Legacy call handling, mapped to a ring score: auto | bar (50) |
        /// call_kit (80). auto = the shell's default for the contact's kind
        #[arg(long = "call-mode", value_name = "MODE")]
        call_mode: Option<String>,
        /// Add from an MCP contact ticket (JSON): fills ref/endpoint fields
        /// and caches the ticket's server/discover offline
        #[arg(long = "mcp-ticket", value_name = "JSON")]
        mcp_ticket: Option<String>,
    },
    /// Update a peer
    Update {
        #[arg(value_name = "REF")]
        peer_ref: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long = "endpoint-id")]
        endpoint_id: Option<String>,
        #[arg(long = "endpoint-addr")]
        endpoint_addr: Option<String>,
        /// Legacy call handling, mapped to a ring score: auto | bar (50) |
        /// call_kit (80). auto = the shell's default for the contact's kind
        #[arg(long = "call-mode", value_name = "MODE")]
        call_mode: Option<String>,
        /// Replace the peer's aliases (repeatable); omit to keep them
        #[arg(long = "alias", value_name = "ALIAS")]
        aliases: Vec<String>,
    },
    /// Manage one peer's concrete devices
    Device {
        #[command(subcommand)]
        command: PeerDeviceCmd,
    },
    /// Remove a peer
    Remove {
        #[arg(value_name = "REF")]
        peer_ref: String,
    },
    /// Resolve a peer reference to live connection info
    Resolve {
        #[arg(value_name = "REF")]
        peer_ref: String,
    },
    /// Show peer details
    Show {
        #[arg(value_name = "REF")]
        peer_ref: String,
    },
    /// Show peer connectivity status
    Status {
        #[arg(value_name = "REF")]
        peer_ref: String,
    },
}

#[derive(Subcommand)]
enum ContactCmd {
    /// Export a vCard 3.0 card carrying the idfon contact ticket
    Export {
        /// Export the saved peer REF's card instead of this identity's own
        #[arg(long)]
        peer: Option<String>,
        /// Write to FILE instead of stdout
        #[arg(long)]
        out: Option<String>,
    },
    /// Add every idfon card in a vCard file as a peer (FILE may be '-' for stdin)
    Import {
        #[arg(value_name = "FILE")]
        file: Option<String>,
    },
}

#[derive(Subcommand)]
enum PeerDeviceCmd {
    /// Add a device endpoint to an existing account peer
    Add {
        #[arg(value_name = "PEER")]
        peer_ref: String,
        #[arg(long = "endpoint-id")]
        endpoint_id: String,
        #[arg(long = "endpoint-addr")]
        endpoint_addr: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long = "device-class")]
        device_class: Option<String>,
        #[arg(long = "capability")]
        capabilities: Vec<String>,
    },
    /// Update an existing device endpoint
    Update {
        #[arg(value_name = "PEER")]
        peer_ref: String,
        #[arg(long = "endpoint-id")]
        endpoint_id: String,
        #[arg(long = "endpoint-addr")]
        endpoint_addr: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long = "device-class")]
        device_class: Option<String>,
        #[arg(long = "capability")]
        capabilities: Vec<String>,
    },
    /// Remove a device endpoint
    Remove {
        #[arg(value_name = "PEER")]
        peer_ref: String,
        #[arg(long = "endpoint-id")]
        endpoint_id: String,
    },
}

#[derive(clap::Args)]
struct SendArgs {
    /// Who to send to: a peer name, alias or id
    #[arg(value_name = "PEER")]
    peer: Option<String>,
    /// The message text (or use --text)
    #[arg(value_name = "MESSAGE", conflicts_with = "text")]
    message: Option<String>,
    /// The message text
    #[arg(long)]
    text: Option<String>,
    /// Send FILE (or stdin when FILE is omitted) instead of text; waits for delivery
    #[arg(long, value_name = "FILE", num_args = 0..=1, default_missing_value = "-")]
    file: Option<String>,
    /// After sending text, wait for the peer's reply and print it
    #[arg(long)]
    wait: bool,
    /// How long --wait waits (default 60s)
    #[arg(long, value_name = "DURATION", value_parser = ux::parse_wait, default_value = "60s")]
    timeout: Duration,
    /// Legacy: use `idfon live publish` / `idfon call`
    #[arg(long, hide = true)]
    stream: bool,
    #[arg(long, hide = true, conflicts_with_all = ["text", "loop_playback"])]
    video: bool,
    #[arg(long, hide = true, conflicts_with_all = ["peer", "text", "file", "stop", "name", "loop_playback"])]
    list: bool,
    #[arg(long, hide = true, conflicts_with_all = ["peer", "text", "file", "list", "name", "loop_playback"])]
    stop: Option<String>,
    #[arg(long, hide = true)]
    seconds: Option<u64>,
    #[arg(long = "loop", hide = true)]
    loop_playback: bool,
    #[arg(long = "no-relay", hide = true)]
    no_relay: bool,
    #[arg(long, hide = true)]
    name: Option<String>,
    /// Thread into a room (prefer `idfon room send`)
    #[arg(long, hide = true)]
    conversation: Option<String>,
    #[arg(long = "idempotency-key")]
    idempotency_key: Option<String>,
    /// Holder-issued capability ticket token (see `idfon access ticket`).
    #[arg(long = "capability-ticket")]
    capability_ticket: Option<String>,
    #[arg(long)]
    retries: Option<u64>,
    /// Delivery policy: failover (default), one, or all
    #[arg(long, value_parser = ["failover", "one", "all"], default_value = "failover")]
    delivery: String,
    /// Restrict delivery to these endpoint IDs (repeatable)
    #[arg(long = "endpoint-id")]
    endpoint_ids: Vec<String>,
    /// Restrict delivery to a device class, e.g. mobile or desktop
    #[arg(long = "device-class")]
    device_class: Option<String>,
    /// Optional per-turn caller context (unsigned, untrusted data): the active
    /// caller-side state the frontend applied, e.g. the speech pipeline.
    /// Peer text only.
    #[arg(long, hide = true, conflicts_with_all = ["file", "stream"])]
    context: Option<String>,
}

#[derive(clap::Args)]
struct RecvArgs {
    /// Only files from this peer (name, alias or id)
    #[arg(long)]
    from: Option<String>,
    /// Save to FILE (default: stdout)
    #[arg(long, short = 'o')]
    out: Option<String>,
    /// Take a file that already arrived within this window (default 2m)
    #[arg(long, value_name = "DURATION", value_parser = ux::parse_wait, default_value = "2m")]
    since: Duration,
    /// Give up waiting after this long (default 60s)
    #[arg(long, value_name = "DURATION", value_parser = ux::parse_wait)]
    timeout: Option<Duration>,
    #[arg(long = "timeout-ms", hide = true)]
    timeout_ms: Option<u64>,
    /// Legacy: use `idfon answer`
    #[arg(long, hide = true)]
    stream: bool,
    #[arg(long, hide = true)]
    seconds: Option<u64>,
    #[arg(long, hide = true)]
    wait: Option<u64>,
    #[arg(long, hide = true)]
    video: bool,
    #[arg(long, hide = true)]
    quality: Option<String>,
    #[arg(long = "no-relay", hide = true)]
    no_relay: bool,
}

#[derive(clap::Args)]
struct InboxArgs {
    /// Only messages from this peer
    #[arg(long)]
    from: Option<String>,
    /// Keep printing new messages as they arrive
    #[arg(long, short = 'f')]
    follow: bool,
    /// Show at most N recent messages
    #[arg(long, short = 'n', default_value_t = 20)]
    limit: usize,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print the home, config file, data dir and socket for this profile
    Path,
    /// Print every setting: its value, where it came from, its env var
    Show,
    /// Save a setting to the config file: `config set edge.url https://…`
    Set {
        /// Key (`edge.url`) or env var name (`IDFON_EDGE_URL`)
        key: String,
        value: String,
    },
    /// Remove a setting from the config file
    Unset {
        /// Key (`edge.url`) or env var name (`IDFON_EDGE_URL`)
        key: String,
    },
}

#[derive(Subcommand)]
enum LiveCmd {
    /// Broadcast an audio FILE (.wav); prints the live ticket and keeps running
    Publish {
        file: String,
        /// Repeat the file forever
        #[arg(long = "loop")]
        loop_playback: bool,
        /// Publish video (adaptive renditions) instead of audio
        #[arg(long)]
        video: bool,
        /// Stable broadcast name
        #[arg(long)]
        name: Option<String>,
        /// Forbid relayed connections
        #[arg(long = "no-relay")]
        no_relay: bool,
    },
    /// List running broadcasts
    List,
    /// Stop a broadcast by id (see `live list`)
    Stop { id: String },
    /// Record a live ticket to FILE (default: stdout)
    Watch {
        ticket: String,
        #[arg(long, short = 'o')]
        out: Option<String>,
        /// Stop after this long (default 15s)
        #[arg(long, value_name = "DURATION", value_parser = ux::parse_wait)]
        duration: Option<Duration>,
        #[arg(long)]
        video: bool,
        /// Video rendition: low, mid, high, highest
        #[arg(long)]
        quality: Option<String>,
        #[arg(long = "no-relay")]
        no_relay: bool,
    },
}

#[derive(clap::Args)]
struct CallArgs {
    /// Who to call
    peer: String,
    /// Audio file to stream (.wav; default: stdin)
    #[arg(long)]
    file: Option<String>,
    /// Hang up after this long
    #[arg(long, value_name = "DURATION", value_parser = ux::parse_wait)]
    duration: Option<Duration>,
    #[arg(long = "no-relay")]
    no_relay: bool,
}

#[derive(clap::Args)]
struct AnswerArgs {
    /// Only answer this peer
    #[arg(long)]
    from: Option<String>,
    /// Record the call to FILE (default: stdout)
    #[arg(long, short = 'o')]
    out: Option<String>,
    /// Give up if nobody calls within this long
    #[arg(long, value_name = "DURATION", value_parser = ux::parse_wait)]
    timeout: Option<Duration>,
    /// Stop recording after this long
    #[arg(long, value_name = "DURATION", value_parser = ux::parse_wait)]
    duration: Option<Duration>,
    #[arg(long)]
    video: bool,
    #[arg(long)]
    quality: Option<String>,
    #[arg(long = "no-relay")]
    no_relay: bool,
}

#[derive(clap::Args)]
struct EventsArgs {
    #[arg(long)]
    follow: bool,
    #[arg(long)]
    after: Option<String>,
    #[arg(long = "type")]
    event_type: Option<String>,
}

#[derive(Subcommand)]
enum RoomCmd {
    /// Create a room: `room create team --member bob`
    Create {
        /// Room name
        #[arg(value_name = "NAME")]
        room_name: Option<String>,
        #[arg(long, hide = true)]
        id: Option<String>,
        #[arg(long, hide = true)]
        name: Option<String>,
        /// Peer ref (id, name, or alias) to add as a member; repeatable
        #[arg(long = "member")]
        members: Vec<String>,
        /// Keep a group call's session open when its last member leaves
        /// (`call.auto_close` off; on by default)
        #[arg(long = "keep-call-open")]
        keep_call_open: bool,
    },
    /// List rooms
    List,
    /// Send a message to every member: `room send team "hi"`
    Send {
        /// Room name or id
        #[arg(value_name = "ROOM")]
        room: String,
        #[arg(value_name = "MESSAGE", conflicts_with = "text", required_unless_present = "text")]
        message: Option<String>,
        #[arg(long)]
        text: Option<String>,
        #[arg(long = "idempotency-key")]
        idempotency_key: Option<String>,
        /// Holder-issued capability ticket token (see `idfon access ticket`).
        #[arg(long = "capability-ticket")]
        capability_ticket: Option<String>,
        #[arg(long)]
        retries: Option<u64>,
        /// Delivery policy applied independently to each room member
        #[arg(long, value_parser = ["failover", "one", "all"], default_value = "failover")]
        delivery: String,
        #[arg(long = "endpoint-id")]
        endpoint_ids: Vec<String>,
        #[arg(long = "device-class")]
        device_class: Option<String>,
    },
    /// Leave a room (local only; other members are not told)
    Leave {
        #[arg(value_name = "ROOM")]
        room: String,
    },
    /// The room's group call (signaling only: media is the app's)
    Call {
        #[command(subcommand)]
        command: RoomCallCmd,
    },
}

#[derive(Subcommand)]
enum RoomCallCmd {
    /// Join the room's call, starting one if none runs (12 people at most)
    Join {
        #[arg(value_name = "ROOM")]
        room: String,
        /// This member's published broadcast ticket
        #[arg(long)]
        ticket: String,
        /// This member's dial address JSON
        #[arg(long)]
        addr: Option<String>,
        #[arg(long = "audio-codec")]
        audio_codec: Option<String>,
    },
    /// Leave the room's call
    Leave {
        #[arg(value_name = "ROOM")]
        room: String,
    },
    /// Show the room's call: session, roster, and whether this member is in it
    Status {
        #[arg(value_name = "ROOM")]
        room: String,
    },
}

#[derive(clap::Args)]
struct WaitArgs {
    #[arg(long = "type")]
    event_type: Option<String>,
    /// Only events after this cursor (default: only events newer than now)
    #[arg(long)]
    after: Option<String>,
    #[arg(long = "timeout-ms")]
    timeout_ms: Option<u64>,
}

#[derive(Subcommand)]
enum OperationCmd {
    /// Fetch operation state
    Get {
        operation_id: String,
        #[arg(long = "timeout-ms")]
        timeout_ms: Option<u64>,
    },
    /// Poll an operation until it reaches a terminal state
    Wait {
        operation_id: String,
        #[arg(long = "timeout-ms")]
        timeout_ms: Option<u64>,
    },
    /// Cancel an operation
    Cancel { operation_id: String },
}

#[derive(Subcommand)]
enum AccessCmd {
    /// Check a capability for a peer: `access check bob message.send`
    Check {
        /// Peer name, alias or id
        peer: Option<String>,
        /// Capability (default message.send: may you message them?)
        capability: Option<String>,
        #[arg(long, hide = true)]
        subject: Option<String>,
        #[arg(long = "capability", hide = true)]
        capability_flag: Option<String>,
    },
    /// Grant yourself a capability for a peer (default: send to and receive from them)
    #[command(alias = "allow")]
    Grant {
        /// Peer name, alias or id
        peer: Option<String>,
        /// Capabilities (default: message.send message.receive)
        capabilities: Vec<String>,
        #[arg(long, hide = true)]
        subject: Option<String>,
        #[arg(long = "capability", hide = true)]
        capability_flag: Option<String>,
    },
    /// Make a ticket that lets PEER message you: `access ticket bob --expires 30d`
    Ticket(TicketArgs),
}

#[derive(Subcommand)]
enum McpCmd {
    /// Open a per-peer local socket, spliced to the peer's idfon/mcp/1 stream
    /// (connect with `idfon-mcp connect --uds <socket>`); requires an
    /// `mcp.transport` grant for the peer
    Listen {
        #[arg(long, value_name = "PEER")]
        to: String,
    },
    /// Set the local MCP server that inbound idfon/mcp/1 streams are spliced
    /// to (persisted; `IDFON_MCP_COMMAND` still overrides per process). Pass
    /// an empty command to disable the relay.
    Configure {
        #[arg(long, value_name = "CMD", default_value = "")]
        command: String,
    },
}

#[derive(clap::Args)]
struct TicketArgs {
    /// Who the ticket is for: peer name, alias or id
    peer: Option<String>,
    #[arg(long, hide = true)]
    subject: Option<String>,
    /// Limit the ticket to one room
    #[arg(long)]
    conversation: Option<String>,
    /// What it allows (default message.receive: the holder may message you)
    #[arg(long, alias = "cap")]
    capability: Option<String>,
    /// Expiry: 2026-12-31, an RFC 3339 time, or a duration like 30d
    #[arg(long, alias = "expires-at", value_parser = ux::parse_expiry)]
    expires: Option<String>,
    /// Bound the ticket to a resource path prefix (e.g. /fs/public)
    #[arg(long = "path-scope")]
    path_scope: Option<String>,
}

#[derive(clap::Args)]
struct PutArgs {
    /// File to store (default: stdin)
    #[arg(value_name = "FILE", conflicts_with = "file")]
    path: Option<String>,
    #[arg(long, hide = true)]
    file: Option<String>,
    #[arg(long = "resource-id")]
    resource_id: Option<String>,
    /// Mime type to tag the resource with (inferred from the file extension
    /// when omitted, e.g. video/mp4 for .mp4)
    #[arg(long)]
    mime: Option<String>,
}

#[derive(clap::Args)]
struct PublishArgs {
    /// File to publish
    file: String,
    /// Stable mutable name, e.g. phonon-31m/bundle.json (content still gets a
    /// hashed URL; the name points at the same bytes)
    #[arg(long)]
    name: Option<String>,
    /// Edge base URL (default IDFON_EDGE_URL on the daemon)
    #[arg(long)]
    url: Option<String>,
    /// Upload token (default IDFON_FILES_TOKEN, then IDFON_EDGE_TOKEN)
    #[arg(long)]
    token: Option<String>,
    /// Rental term, e.g. 30d, 12h, 90m (the edge may cap it). User publishes
    /// default to 24h; without a paid tier a user pin cannot exceed that.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    expires_in: Option<u64>,
    /// Operational file: never expires (requires the edge operations token,
    /// IDFON_FILES_OPS_TOKEN).
    #[arg(long, conflicts_with = "expires_in")]
    permanent: bool,
}

/// `30d`, `12h`, `90m`, `60s`; a bare number is seconds.
fn parse_duration(value: &str) -> Result<u64, String> {
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let (digits, unit) = value.split_at(split);
    let amount: u64 = digits
        .parse()
        .map_err(|_| format!("invalid duration {value:?}"))?;
    let scale = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return Err(format!("invalid duration unit {unit:?} (use s, m, h, d, w)")),
    };
    amount
        .checked_mul(scale)
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| format!("invalid duration {value:?}"))
}

#[derive(clap::Args)]
struct GetArgs {
    ticket: String,
    /// Save to FILE (default: stdout)
    #[arg(long, short = 'o')]
    out: Option<String>,
    /// Live tickets only: cap the capture window (seconds)
    #[arg(long)]
    seconds: Option<u64>,
    /// Live tickets only: record encoded video (Annex B .h264) instead of audio
    #[arg(long)]
    video: bool,
    /// Video only: rendition quality (low, mid, high, highest)
    #[arg(long)]
    quality: Option<String>,
    /// Live tickets only: forbid relayed connections
    #[arg(long = "no-relay")]
    no_relay: bool,
}

fn main() {
    // Shared subscriber on stderr; stdout stays reserved for the CLI's output
    // contracts. Conservative default so borrowed iroh/daemon noise stays quiet.
    idfon_telemetry::init("idfon", "warn");
    if let Err(error) = run() {
        eprintln!("idfon: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let cli = Cli::parse();
    KEEP_ALIVE.set(cli.keep_alive).expect("run called once");
    let mut relay_args = Vec::new();
    if let Some(urls) = &cli.relay_urls {
        relay_args.extend(["--relay-urls".to_string(), urls.clone()]);
    }
    if let Some(token) = &cli.relay_token {
        relay_args.extend(["--relay-token".to_string(), token.clone()]);
    }
    if cli.relay_only {
        relay_args.push("--relay-only".to_string());
    }
    RELAY_ARGS.set(relay_args).expect("run called once");
    let profile = idfon_config::paths::profile(cli.profile.as_deref());
    PROFILE.set(profile.clone()).expect("run called once");
    let default_socket = idfon_config::paths::socket(profile.as_deref())
        .display()
        .to_string();
    let socket: &str = cli.socket.as_deref().unwrap_or(&default_socket);
    let identity = cli.identity.as_deref();
    let json = cli.json;

    match cli.command {
        Command::Shutdown => {
            // No auto-start here: shutting down a daemon we just spawned is a no-op.
            let mut client = Client::connect(socket)
                .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "daemon is not running"))?;
            let request = Request::call("cli-shutdown", &rpc::DaemonShutdown {});
            let response = client
                .request(&encode_json(&request).map_err(io::Error::other)?)
                .map_err(io::Error::other)?
                .json()
                .map_err(io::Error::other)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&response).map_err(io::Error::other)?
                );
            } else {
                println!("daemon stopped");
            }
            Ok(())
        }
        Command::Contact(ContactCmd::Export { peer, out }) => {
            cmd_contact_export(socket, identity, peer.as_deref(), out.as_deref(), json)
        }
        Command::Contact(ContactCmd::Import { file }) => {
            cmd_contact_import(socket, identity, file.as_deref(), json)
        }
        Command::Put(args) => cmd_put(
            socket,
            args.path.or(args.file).as_ref(),
            args.resource_id,
            args.mime.as_ref(),
            json,
            identity,
        ),
        Command::Publish(args) => {
            // The daemon may run with another cwd; hand it an absolute path.
            let path = std::fs::canonicalize(&args.file).map_err(|error| {
                io::Error::new(error.kind(), format!("cannot read {}: {error}", args.file))
            })?;
            let params = rpc::FilePublish {
                path: path.display().to_string(),
                name: args.name,
                url: args.url,
                token: args.token,
                expires_in: args.expires_in,
                permanent: args.permanent,
            };
            let response = call(socket, &params, identity, false)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&response).map_err(io::Error::other)?
                );
                return Ok(());
            }
            if response.ok {
                println!("{}", result_str(&response, "url"));
                Ok(())
            } else {
                finish(response, false)
            }
        }
        Command::Get(args) => {
            // Tickets are self-describing: live tickets carry an "ilv1:" (or legacy "iroh-live:")
            // scheme prefix, blob tickets start with "blob".
            if args.ticket.starts_with("ilv1:") || args.ticket.starts_with("iroh-live:") {
                cmd_listen(
                    socket,
                    &args.ticket,
                    args.out.as_ref(),
                    args.seconds,
                    args.video,
                    args.quality.as_deref(),
                    !args.no_relay,
                    json,
                    identity,
                )
            } else {
                cmd_get(socket, &args.ticket, args.out.as_ref(), identity)
            }
        }
        Command::Gateway(GatewayCmd::Start) => cmd_gateway_start(socket, identity, json),
        Command::Gateway(GatewayCmd::Stop) => finish(
            call(socket, &rpc::GatewayStop {}, identity, false)?,
            json,
        ),
        Command::Provider(ProviderCmd::Start { root }) => finish(
            call(socket, &rpc::ProviderStart { root }, identity, false)?,
            json,
        ),
        Command::Provider(ProviderCmd::Stop) => finish(
            call(socket, &rpc::ProviderStop {}, identity, false)?,
            json,
        ),
        Command::Fetch(args) => {
            cmd_fetch(socket, &args.url, args.out.as_ref(), identity, &args.prefer)
        }
        Command::Web(WebCmd::Open { target }) => cmd_web_open(socket, &target, identity),
        Command::Recv(args) => {
            if args.stream {
                cmd_answer(
                    socket,
                    args.out.as_ref(),
                    args.seconds,
                    args.wait,
                    args.from.as_deref(),
                    args.video,
                    args.quality.as_deref(),
                    !args.no_relay,
                    json,
                    identity,
                )
            } else {
                cmd_recv(
                    socket,
                    args.out.as_ref(),
                    args.from.as_deref(),
                    args.since,
                    args.timeout
                        .or(args.timeout_ms.map(Duration::from_millis))
                        .unwrap_or(Duration::from_secs(60)),
                    identity,
                )
            }
        }
        Command::Events(args) => cmd_events(
            socket,
            args.follow,
            args.after,
            args.event_type,
            json,
            identity,
        ),
        Command::Wait(args) => {
            // Default: only events newer than now (an old match would return
            // at once and look like a fresh one).
            let after = args.after.or_else(|| ux::latest_cursor(socket, identity));
            let params = rpc::EventWait {
                after,
                event_type: args.event_type,
                timeout_ms: args.timeout_ms,
                ..Default::default()
            };
            let response = call(socket, &params, identity, cli.stdin_json)?;
            finish(response, json)
        }
        Command::Operation(OperationCmd::Wait {
            operation_id,
            timeout_ms,
        }) => {
            let request = request_for(
                &rpc::OperationWait {
                    operation_id,
                    timeout_ms,
                },
                identity,
            );
            let mut client = connect_or_start_daemon(socket)?;
            let payload = encode_json(&request).map_err(io::Error::other)?;
            let mut response = read_json(&mut client, &payload)?;
            loop {
                let terminal = match &response.body {
                    ResponseBody::Success { result, .. } => result
                        .get("operation")
                        .and_then(|value| value.get("status"))
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|status| {
                            matches!(status, "delivered" | "failed" | "expired" | "cancelled")
                        }),
                    _ => true,
                };
                if terminal {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                response = read_json(&mut client, &payload)?;
            }
            finish(response, json)
        }
        Command::Status => finish(
            call(socket, &rpc::DaemonStatus {}, identity, cli.stdin_json)?,
            json,
        ),
        Command::Doctor => cmd_doctor(socket, identity, json, cli.stdin_json),
        Command::Identity(IdentityCmd::List) => {
            let response = call(socket, &rpc::IdentityList {}, identity, cli.stdin_json)?;
            if json || !response.ok {
                return finish(response, json);
            }
            for item in ux::ok_result(response)?["identities"].as_array().into_iter().flatten() {
                let name = item["name"].as_str().unwrap_or("?");
                let id = item["id"].as_str().unwrap_or("?");
                let active = if item["active"] == true { "* " } else { "  " };
                if name == id {
                    println!("{active}{name}");
                } else {
                    println!("{active}{name}  (id {id})");
                }
            }
            Ok(())
        }
        Command::Identity(IdentityCmd::Use { name }) => finish(
            call(socket, &rpc::IdentityUse { name }, identity, cli.stdin_json)?,
            json,
        ),
        Command::Identity(IdentityCmd::Create { name }) => finish(
            call(socket, &rpc::IdentityCreate { name }, identity, cli.stdin_json)?,
            json,
        ),
        Command::Identity(IdentityCmd::Delete { name }) => finish(
            call(socket, &rpc::IdentityDelete { name }, identity, cli.stdin_json)?,
            json,
        ),
        Command::Peer(PeerCmd::List) => {
            let response = call(socket, &rpc::PeerList {}, identity, cli.stdin_json)?;
            if json || !response.ok {
                return finish(response, json);
            }
            let peers = ux::ok_result(response)?["peers"].as_array().cloned().unwrap_or_default();
            if peers.is_empty() {
                eprintln!("no contacts yet (add one with `idfon peer add carol.vcf`)");
            }
            for peer in peers {
                println!(
                    "{}  {}",
                    peer["name"].as_str().unwrap_or("?"),
                    peer["id"].as_str().unwrap_or("?").chars().take(12).collect::<String>()
                );
            }
            Ok(())
        }
        Command::Peer(PeerCmd::Add {
            peer_ref,
            account_id,
            name,
            endpoint_id,
            endpoint_addr,
            call_mode,
            mcp_ticket,
        }) => {
            // One `add` for every way a contact travels: a .vcf card, an
            // idfon://add link, an MCP contact ticket, or explicit fields.
            let input = peer_ref.as_deref().unwrap_or_default();
            let is_card = input == "-"
                || input.to_ascii_lowercase().ends_with(".vcf")
                || std::fs::read_to_string(input).is_ok_and(|text| text.contains("BEGIN:VCARD"));
            if is_card {
                return cmd_contact_import(socket, identity, Some(input), json);
            }
            let params = if input.starts_with("idfon://add/") {
                let ticket =
                    idfon_protocol::vcf::contact_ticket_from_link(input).map_err(io::Error::other)?;
                rpc::PeerAdd {
                    name: name.or_else(|| ticket.label.clone()),
                    contact_ticket: Some(ticket),
                    ..Default::default()
                }
            } else if input.trim_start().starts_with('{') && mcp_ticket.is_none() {
                rpc::PeerAdd {
                    name,
                    mcp_ticket: Some(input.to_owned()),
                    ..Default::default()
                }
            } else {
                rpc::PeerAdd {
                    id: account_id.or_else(|| peer_ref.clone()),
                    peer: peer_ref,
                    name,
                    endpoint_id,
                    endpoint_addr,
                    aliases: None,
                    call_mode,
                    mcp_ticket,
                    contact_ticket: None,
                }
            };
            print_added(call(socket, &params, identity, cli.stdin_json)?, json)
        }
        Command::Peer(PeerCmd::Update {
            peer_ref,
            name,
            endpoint_id,
            endpoint_addr,
            call_mode,
            aliases,
        }) => finish(
            call(
                socket,
                &rpc::PeerUpdate {
                    id: Some(peer_ref.clone()),
                    peer: peer_ref,
                    name,
                    endpoint_id,
                    endpoint_addr,
                    // Absent = keep: the daemon replaces aliases whenever the
                    // param is present (an empty list used to wipe them).
                    aliases: (!aliases.is_empty()).then_some(aliases),
                    call_mode,
                },
                identity,
                cli.stdin_json,
            )?,
            json,
        ),
        Command::Peer(PeerCmd::Device { command }) => {
            let response = match command {
                PeerDeviceCmd::Add {
                    peer_ref,
                    endpoint_id,
                    endpoint_addr,
                    label,
                    device_class,
                    capabilities,
                } => call(
                    socket,
                    &rpc::PeerDeviceAdd {
                        peer: peer_ref,
                        endpoint_id,
                        endpoint_addr,
                        label,
                        device_class,
                        capabilities,
                    },
                    identity,
                    cli.stdin_json,
                )?,
                PeerDeviceCmd::Update {
                    peer_ref,
                    endpoint_id,
                    endpoint_addr,
                    label,
                    device_class,
                    capabilities,
                } => call(
                    socket,
                    &rpc::PeerDeviceUpdate {
                        peer: peer_ref,
                        endpoint_id,
                        endpoint_addr,
                        label,
                        device_class,
                        capabilities,
                    },
                    identity,
                    cli.stdin_json,
                )?,
                PeerDeviceCmd::Remove {
                    peer_ref,
                    endpoint_id,
                } => call(
                    socket,
                    &rpc::PeerDeviceRemove {
                        peer: peer_ref,
                        endpoint_id,
                    },
                    identity,
                    cli.stdin_json,
                )?,
            };
            finish(response, json)
        }
        Command::Peer(PeerCmd::Remove { peer_ref }) => finish(
            call(socket, &rpc::PeerRemove { peer: peer_ref }, identity, cli.stdin_json)?,
            json,
        ),
        Command::Peer(PeerCmd::Resolve { peer_ref }) => finish(
            call(socket, &rpc::PeerResolve { peer: peer_ref }, identity, cli.stdin_json)?,
            json,
        ),
        Command::Peer(PeerCmd::Show { peer_ref }) => finish(
            call(socket, &rpc::PeerShow { peer: peer_ref }, identity, cli.stdin_json)?,
            json,
        ),
        Command::Peer(PeerCmd::Status { peer_ref }) => finish(
            call(socket, &rpc::PeerStatus { peer: peer_ref }, identity, cli.stdin_json)?,
            json,
        ),
        Command::Room(command) => match command {
            RoomCmd::Create { room_name, id, name, members, keep_call_open } => {
                let params = rpc::RoomCreate {
                    id,
                    name: room_name.or(name),
                    members,
                    call_auto_close: keep_call_open.then_some(false),
                };
                let response = call(socket, &params, identity, cli.stdin_json)?;
                if json || !response.ok {
                    return finish(response, json);
                }
                let room = &ux::ok_result(response)?["room"];
                let others = room["members"].as_array().map_or(0, Vec::len);
                println!(
                    "created room {} (you + {others} other{})",
                    room["name"].as_str().or(room["id"].as_str()).unwrap_or("?"),
                    if others == 1 { "" } else { "s" }
                );
                Ok(())
            }
            RoomCmd::List => finish(
                call(socket, &rpc::RoomList {}, identity, cli.stdin_json)?,
                json,
            ),
            RoomCmd::Send {
                room,
                message,
                text,
                idempotency_key,
                capability_ticket,
                retries,
                delivery,
                endpoint_ids,
                device_class,
            } => {
                let key = idempotency_key.unwrap_or_else(|| {
                    format!(
                        "idfon-room-{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("clock before epoch")
                            .as_nanos()
                    )
                });
                {
                let room_ref = room.clone();
                let response = call(
                        socket,
                        &rpc::RoomSend {
                            room: ux::resolve_room(socket, identity, &room)?,
                            text: message.or(text).unwrap_or_default(),
                            idempotency_key: key,
                            capability_ticket,
                            retries,
                            delivery: Some(delivery_policy(&delivery, endpoint_ids, device_class)),
                        },
                        identity,
                        cli.stdin_json,
                    )?;
                if json || !response.ok {
                    return finish(response, json);
                }
                let result = ux::ok_result(response)?;
                let failures = result["failures"].as_array().map_or(0, Vec::len);
                println!(
                    "sent to {room_ref} ({} delivered{})",
                    result["operation_ids"].as_array().map_or(0, Vec::len),
                    if failures > 0 { format!(", {failures} failed") } else { String::new() }
                );
                Ok(())
                }
            }
            RoomCmd::Leave { room } => finish(
                call(socket, &rpc::RoomLeave { room }, identity, cli.stdin_json)?,
                json,
            ),
            RoomCmd::Call { command } => {
                let stdin_json = cli.stdin_json;
                let response = match command {
                    RoomCallCmd::Join { room, ticket, addr, audio_codec } => call(
                        socket,
                        &rpc::RoomCallJoin { room, ticket, addr, audio_codec },
                        identity,
                        stdin_json,
                    )?,
                    RoomCallCmd::Leave { room } => {
                        call(socket, &rpc::RoomCallLeave { room }, identity, stdin_json)?
                    }
                    RoomCallCmd::Status { room } => {
                        call(socket, &rpc::RoomCallStatus { room }, identity, stdin_json)?
                    }
                };
                finish(response, json)
            }
        },
        Command::Send(args) => {
            if args.stream {
                match (args.peer.as_deref(), args.list, args.stop.as_deref()) {
                    (Some(peer), false, None) => {
                        // 1:1 session-scoped stream to the peer; blocks until
                        // they hang up (or --seconds). No ticket exists.
                        let file = resolve_stream_file(args.file.as_ref())?;
                        let params = rpc::LiveDial {
                            peer: peer.to_owned(),
                            file,
                            video: args.video,
                            relay: !args.no_relay,
                            seconds: args.seconds,
                            delivery: Some(delivery_policy(
                                &args.delivery,
                                args.endpoint_ids.clone(),
                                args.device_class.clone(),
                            )),
                        };
                        let response = call_plain(socket, &params, identity)?;
                        if !response.ok {
                            return Err(io::Error::other(request_error(&response)));
                        }
                        if json {
                            println!(
                                "{}",
                                serde_json::to_string(&response).map_err(io::Error::other)?
                            );
                        } else {
                            let held = result_str(&response, "held_ms");
                            println!("streamed to {peer} for {held} ms");
                        }
                        Ok(())
                    }
                    (None, true, _) => finish(
                        call(socket, &rpc::LiveList {}, identity, cli.stdin_json)?,
                        json,
                    ),
                    (None, false, Some(id)) => {
                        let response =
                            call_plain(socket, &rpc::LiveStop { id: id.to_owned() }, identity)?;
                        if !response.ok {
                            return Err(io::Error::other(request_error(&response)));
                        }
                        if json {
                            println!(
                                "{}",
                                serde_json::to_string(&response).map_err(io::Error::other)?
                            );
                        } else {
                            println!("stopped {}", result_str(&response, "id"));
                        }
                        Ok(())
                    }
                    (None, false, None) => {
                        // Broadcast publish: prints the live ticket.
                        if args.video {
                            let file = resolve_stream_file(args.file.as_ref())?;
                            let params = rpc::LivePublish {
                                file,
                                video: true,
                                relay: !args.no_relay,
                                name: args.name.clone(),
                                ..Default::default()
                            };
                            let response = call_plain(socket, &params, identity)?;
                            if !response.ok {
                                return Err(io::Error::other(request_error(&response)));
                            }
                            if json {
                                println!(
                                    "{}",
                                    serde_json::to_string(&response).map_err(io::Error::other)?
                                );
                                return Ok(());
                            }
                            println!("{}", result_str(&response, "ticket"));
                            return Ok(());
                        }
                        let file = resolve_stream_file(args.file.as_ref())?;
                        let params = rpc::LivePublish {
                            file,
                            loop_playback: args.loop_playback,
                            relay: !args.no_relay,
                            name: args.name.clone(),
                            ..Default::default()
                        };
                        let response = call_plain(socket, &params, identity)?;
                        if !response.ok {
                            return Err(io::Error::other(request_error(&response)));
                        }
                        if json {
                            println!(
                                "{}",
                                serde_json::to_string(&response).map_err(io::Error::other)?
                            );
                            return Ok(());
                        }
                        println!("{}", result_str(&response, "ticket"));
                        Ok(())
                    }
                    _ => Err(io::Error::other(
                        "--list/--stop cannot be combined with a PEER",
                    )),
                }
            } else {
                let Some(peer) = args.peer.as_deref() else {
                    return Err(io::Error::other(
                        "PEER required without --stream (broadcast publish is send --stream)",
                    ));
                };
                send_payload(socket, cli.stdin_json, &args, peer, json, identity)
            }
        }
        Command::Operation(OperationCmd::Get {
            operation_id,
            timeout_ms,
        }) => finish(
            call(
                socket,
                &rpc::OperationGet {
                    operation_id,
                    timeout_ms,
                },
                identity,
                cli.stdin_json,
            )?,
            json,
        ),
        Command::Operation(OperationCmd::Cancel { operation_id }) => finish(
            call(
                socket,
                &rpc::OperationCancel { operation_id },
                identity,
                cli.stdin_json,
            )?,
            json,
        ),
        Command::Access(AccessCmd::Check {
            peer,
            capability,
            subject,
            capability_flag,
        }) => {
            let reference = peer
                .or(subject)
                .ok_or_else(|| io::Error::other("usage: idfon access check PEER [CAPABILITY]"))?;
            let capability = capability
                .or(capability_flag)
                .unwrap_or_else(|| "message.send".into());
            let response = call(
                socket,
                &rpc::AccessCheck {
                    peer: Some(ux::peer_key(socket, identity, &reference)?),
                    capability: Some(capability.clone()),
                },
                identity,
                cli.stdin_json,
            )?;
            if json || !response.ok {
                return finish(response, json);
            }
            let allowed = ux::ok_result(response)?["allowed"] == true;
            println!(
                "{}: {capability} is {}",
                reference,
                if allowed { "allowed" } else { "not allowed" }
            );
            Ok(())
        }
        Command::Access(AccessCmd::Grant {
            peer,
            capabilities,
            subject,
            capability_flag,
        }) => {
            let reference = peer
                .or(subject)
                .ok_or_else(|| io::Error::other("usage: idfon access grant PEER [CAPABILITY...]"))?;
            let key = ux::peer_key(socket, identity, &reference)?;
            let mut capabilities: Vec<String> =
                capabilities.into_iter().chain(capability_flag).collect();
            if capabilities.is_empty() {
                capabilities = vec!["message.send".into(), "message.receive".into()];
            }
            for capability in &capabilities {
                let response = call(
                    socket,
                    &rpc::AccessGrant {
                        peer: key.clone(),
                        capability: capability.clone(),
                    },
                    identity,
                    cli.stdin_json,
                )?;
                if json || !response.ok {
                    finish(response, json)?;
                } else {
                    println!("granted {capability} for {reference}");
                }
            }
            Ok(())
        }
        Command::Mcp(McpCmd::Listen { to }) => finish(
            call(socket, &rpc::McpListen { peer: to }, identity, cli.stdin_json)?,
            json,
        ),
        Command::Mcp(McpCmd::Configure { command }) => finish(
            call(socket, &rpc::McpConfigure { command }, identity, cli.stdin_json)?,
            json,
        ),
        Command::Access(AccessCmd::Ticket(args)) => {
            let reference = args
                .peer
                .or(args.subject)
                .ok_or_else(|| io::Error::other("usage: idfon access ticket PEER [--expires 30d]"))?;
            let params = rpc::AccessTicket {
                peer: ux::peer_key(socket, identity, &reference)?,
                capabilities: vec![args
                    .capability
                    .clone()
                    .unwrap_or_else(|| "message.receive".into())],
                expires_at: args.expires,
                conversation: args.conversation,
                path_scope: args.path_scope,
            };
            let response = call(socket, &params, identity, cli.stdin_json)?;
            if json || !response.ok {
                return finish(response, json);
            }
            match &ux::ok_result(response)?["ticket"] {
                Value::String(ticket) => println!("{ticket}"),
                other => println!("{other}"),
            }
            Ok(())
        }
        Command::Config(ConfigCmd::Path) => {
            let profile = PROFILE.get().cloned().flatten();
            let rows = [
                ("home", idfon_config::paths::home()),
                ("config", idfon_config::paths::config_file()),
                ("data", idfon_config::paths::data_dir(profile.as_deref())),
                ("socket", std::path::PathBuf::from(socket)),
            ];
            if json {
                let map: serde_json::Map<String, Value> = rows
                    .iter()
                    .map(|(name, path)| (name.to_string(), json!(path.display().to_string())))
                    .collect();
                println!("{}", Value::Object(map));
            } else {
                for (name, path) in rows {
                    println!("{name:<7}{}", path.display());
                }
            }
            Ok(())
        }
        Command::Config(ConfigCmd::Set { key, value }) => {
            let path = idfon_config::set(&key, &value).map_err(io::Error::other)?;
            let setting = idfon_config::find(&key).expect("set validated the key");
            println!("set {} in {}", setting.key, path.display());
            note_config_overrides(setting);
            Ok(())
        }
        Command::Config(ConfigCmd::Unset { key }) => {
            let path = idfon_config::unset(&key).map_err(io::Error::other)?;
            let setting = idfon_config::find(&key).expect("unset validated the key");
            println!("removed {} from {}", setting.key, path.display());
            note_config_overrides(setting);
            Ok(())
        }
        Command::Config(ConfigCmd::Show) => {
            let mut rows = Vec::new();
            for setting in idfon_config::SETTINGS {
                let (value, source) = idfon_config::lookup(setting.env);
                let shown = match (&value, setting.secret) {
                    (Some(_), true) => "********".to_owned(),
                    (Some(value), false) => value.clone(),
                    (None, _) => "-".to_owned(),
                };
                let source = match source {
                    idfon_config::Source::Env => "env",
                    idfon_config::Source::File => "config",
                    idfon_config::Source::Unset => "unset",
                };
                rows.push((setting, shown, source));
            }
            if json {
                let list: Vec<Value> = rows
                    .iter()
                    .map(|(setting, shown, source)| {
                        json!({"key": setting.key, "env": setting.env, "value": shown, "source": source, "help": setting.help})
                    })
                    .collect();
                println!("{}", Value::Array(list));
            } else {
                println!("# {}  (env vars override it)", idfon_config::paths::config_file().display());
                for (setting, shown, source) in rows {
                    println!("{:<22} {:<28} {:<6} {}", setting.key, shown, source, setting.env);
                }
            }
            Ok(())
        }
        Command::Inbox(args) => {
            ux::cmd_inbox(socket, identity, args.from.as_deref(), args.limit, args.follow, json)
        }
        Command::Live(command) => match command {
            LiveCmd::Publish {
                file,
                loop_playback,
                video,
                name,
                no_relay,
            } => {
                let params = rpc::LivePublish {
                    file: ux::daemon_path(&file)?,
                    loop_playback,
                    video,
                    relay: !no_relay,
                    name,
                };
                let response = call_plain(socket, &params, identity)?;
                if json || !response.ok {
                    return finish(response, json);
                }
                let result = ux::ok_result(response)?;
                println!("{}", result["ticket"].as_str().unwrap_or_default());
                eprintln!(
                    "broadcasting as {} (stop with `idfon live stop {}`)",
                    result["id"].as_str().unwrap_or("?"),
                    result["id"].as_str().unwrap_or("ID")
                );
                Ok(())
            }
            LiveCmd::List => {
                let response = call_plain(socket, &rpc::LiveList {}, identity)?;
                if json || !response.ok {
                    return finish(response, json);
                }
                let result = ux::ok_result(response)?;
                let publishers = result["publishers"].as_array().cloned().unwrap_or_default();
                if publishers.is_empty() {
                    eprintln!("no broadcasts running");
                }
                for publisher in publishers {
                    println!(
                        "{}  {}  {}",
                        publisher["id"].as_str().unwrap_or("?"),
                        publisher["kind"].as_str().unwrap_or("?"),
                        publisher["ticket"].as_str().unwrap_or("")
                    );
                }
                Ok(())
            }
            LiveCmd::Stop { id } => {
                let response = call_plain(socket, &rpc::LiveStop { id: id.clone() }, identity)?;
                if json || !response.ok {
                    return finish(response, json);
                }
                println!("stopped {id}");
                Ok(())
            }
            LiveCmd::Watch {
                ticket,
                out,
                duration,
                video,
                quality,
                no_relay,
            } => cmd_listen(
                socket,
                &ticket,
                out.as_ref(),
                duration.map(|d| d.as_secs()),
                video,
                quality.as_deref(),
                !no_relay,
                json,
                identity,
            ),
        },
        Command::Call(args) => {
            let file = match args.file.as_deref() {
                Some(path) if path != "-" => ux::daemon_path(path)?,
                other => resolve_stream_file(other.map(str::to_owned).as_ref())?,
            };
            let params = rpc::LiveDial {
                peer: args.peer.clone(),
                file,
                video: false,
                relay: !args.no_relay,
                seconds: args.duration.map(|d| d.as_secs()),
                delivery: Some(DeliveryPolicy::default()),
            };
            let response = call_plain(socket, &params, identity)?;
            if json || !response.ok {
                return finish(response, json);
            }
            println!(
                "call with {} ended after {} ms",
                args.peer,
                result_str(&response, "held_ms")
            );
            Ok(())
        }
        Command::Answer(args) => cmd_answer(
            socket,
            args.out.as_ref(),
            args.duration.map(|d| d.as_secs()),
            args.timeout.map(|d| d.as_secs()),
            args.from.as_deref(),
            args.video,
            args.quality.as_deref(),
            !args.no_relay,
            json,
            identity,
        ),
        Command::Rpc { method, params } => {
            let params: Value = match params.as_deref() {
                None => json!({}),
                Some("-") => serde_json::from_reader(io::stdin()).map_err(io::Error::other)?,
                Some(text) => serde_json::from_str(text)
                    .map_err(|error| io::Error::other(format!("params must be JSON: {error}")))?,
            };
            let mut request = Request {
                version: idfon_protocol::PROTOCOL_VERSION,
                id: "cli-rpc".into(),
                method,
                params,
            };
            if let (Some(identity), Some(object)) = (identity, request.params.as_object_mut()) {
                object
                    .entry("identity")
                    .or_insert(Value::String(identity.into()));
            }
            let mut client = connect_or_start_daemon(socket)?;
            let response = read_json(
                &mut client,
                &encode_json(&request).map_err(io::Error::other)?,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&response).map_err(io::Error::other)?
            );
            if response.ok {
                Ok(())
            } else {
                Err(io::Error::other("request failed"))
            }
        }
    }
}

/// Exports a vCard 3.0 card: this identity's own ticket (`contact.ticket`),
/// or the saved peer REF's primary pair from `peers`, wrapped in `X-IDFON-TICKET`.
fn cmd_contact_export(
    socket: &str,
    identity: Option<&str>,
    peer_ref: Option<&str>,
    out: Option<&str>,
    json: bool,
) -> io::Result<()> {
    let ticket = match peer_ref {
        None => {
            let response = call(socket, &rpc::ContactTicket {}, identity, false)?;
            let result = match response.body {
                ResponseBody::Success { result, .. } => result,
                _ => return finish(response, json),
            };
            serde_json::from_value(result).map_err(io::Error::other)?
        }
        Some(reference) => {
            let response = call(socket, &rpc::PeerList {}, identity, false)?;
            let result = match response.body {
                ResponseBody::Success { result, .. } => result,
                _ => return finish(response, json),
            };
            let peers: Vec<Peer> =
                serde_json::from_value(result["peers"].clone()).map_err(io::Error::other)?;
            let peer = peers
                .into_iter()
                .find(|peer| peer.id == reference || peer.name == reference)
                .ok_or_else(|| io::Error::other(format!("no such peer: {reference}")))?;
            let (Some(endpoint_id), Some(endpoint_addr)) =
                (peer.endpoint_id.clone(), peer.endpoint_addr.clone())
            else {
                return Err(io::Error::other("peer has no endpoint address"));
            };
            ContactTicket {
                version: 1,
                account_id: peer.id,
                endpoint_id,
                endpoint_addr,
                label: Some(peer.name),
                device_class: None,
                capabilities: vec![],
            }
        }
    };
    let vcard = contact_ticket_vcard(&ticket);
    match out {
        Some(path) => std::fs::write(path, vcard)?,
        None => print!("{vcard}"),
    }
    Ok(())
}

/// Imports every idfon card in a vCard file as a peer (`peer.add` per card).
fn cmd_contact_import(
    socket: &str,
    identity: Option<&str>,
    file: Option<&str>,
    json: bool,
) -> io::Result<()> {
    let text = match file {
        None | Some("-") => {
            let mut text = String::new();
            io::Read::read_to_string(&mut io::stdin(), &mut text)?;
            text
        }
        Some(path) => std::fs::read_to_string(path)?,
    };
    for ticket in vcard_contact_tickets(&text).map_err(io::Error::other)? {
        let name = ticket.label.clone();
        let params = rpc::PeerAdd {
            contact_ticket: Some(ticket),
            name,
            ..Default::default()
        };
        let response = call(socket, &params, identity, false)?;
        print_added(response, json)?;
    }
    Ok(())
}

/// After a config change: who still wins over the file, and when it applies.
fn note_config_overrides(setting: &idfon_config::Setting) {
    if std::env::var(setting.env).is_ok_and(|value| !value.trim().is_empty()) {
        eprintln!("note: {} is set in your environment and overrides the file", setting.env);
    }
    eprintln!("a running daemon reads the file at start: `idfon shutdown` to apply");
}

/// `added NAME`, or the envelope with `--json`, or the error.
fn print_added(response: Response, json: bool) -> io::Result<()> {
    if json || !response.ok {
        return finish(response, json);
    }
    let result = ux::ok_result(response)?;
    println!(
        "added {}",
        result["peer"]["name"].as_str().unwrap_or("contact")
    );
    Ok(())
}

/// Prints the response (JSON envelope with `--json`, human text otherwise)
/// and maps `ok: false` onto a non-zero exit.
fn finish(response: Response, json: bool) -> io::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string(&response).map_err(io::Error::other)?
        );
    } else {
        print_human(&response);
    }
    if response.ok {
        Ok(())
    } else {
        Err(io::Error::other("request failed"))
    }
}

/// Binds the identity's loopback gateway and prints URL + token (or the full
/// envelope with `--json`).
fn cmd_gateway_start(socket: &str, identity: Option<&str>, json: bool) -> io::Result<()> {
    let response = call(socket, &rpc::GatewayStart::default(), identity, false)?;
    let result = match &response.body {
        ResponseBody::Success { result, .. } => result.clone(),
        _ => return finish(response, json),
    };
    if json {
        return finish(response, true);
    }
    println!("gateway {}", result["url"].as_str().unwrap_or_default());
    println!("token   {}", result["token"].as_str().unwrap_or_default());
    Ok(())
}

/// Fetches an `idfon://<account>/<path>` resource through the local gateway,
/// either to stdout or `--out FILE`.
fn cmd_fetch(
    socket: &str,
    url: &str,
    out: Option<&String>,
    identity: Option<&str>,
    prefer: &str,
) -> io::Result<()> {
    use std::io::Write as _;

    let (account, path) = split_idfon_url(url)?;
    let params = rpc::GatewayStart {
        prefer: Some(prefer.to_owned()),
    };
    let response = call(socket, &params, identity, false)?;
    let result = match &response.body {
        ResponseBody::Success { result, .. } => result,
        _ => return finish(response, false),
    };
    let addr = result["addr"].as_str().unwrap_or_default();
    let token = result["token"].as_str().unwrap_or_default();
    let (status, body) = http_get(addr, &format!("/{account}{path}"), token)?;
    if status != 200 {
        return Err(io::Error::other(format!(
            "gateway returned {status}: {}",
            String::from_utf8_lossy(&body).trim()
        )));
    }
    match out {
        Some(file) => std::fs::write(file, &body)?,
        None => io::stdout().write_all(&body)?,
    }
    Ok(())
}

/// Prints the public edge URL for `<ref>/<path>` via the daemon.
fn cmd_web_open(socket: &str, target: &str, identity: Option<&str>) -> io::Result<()> {
    let (reference, path) = match target.split_once('/') {
        Some((reference, path)) => (reference.to_owned(), format!("/{path}")),
        None => (target.to_owned(), "/".to_owned()),
    };
    if reference.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "missing ref"));
    }
    let params = rpc::WebUrl {
        peer: reference,
        path,
    };
    let response = call(socket, &params, identity, false)?;
    match &response.body {
        ResponseBody::Success { result, .. } => {
            println!("{}", result["url"].as_str().unwrap_or_default());
            Ok(())
        }
        _ => finish(response, false),
    }
}

/// Splits `idfon://<account>/<path>` into the account and a leading-slash path.
fn split_idfon_url(url: &str) -> io::Result<(String, String)> {
    let rest = url.strip_prefix("idfon://").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected idfon://<account>/<path>",
        )
    })?;
    let (account, path) = match rest.split_once('/') {
        Some((account, path)) => (account, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if account.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing account",
        ));
    }
    Ok((account.to_string(), path))
}

/// Minimal loopback HTTP/1.1 GET. The gateway answers with a known-length body
/// (hyper `Full`), so there is no chunked decoding to do.
fn http_get(addr: &str, path: &str, token: &str) -> io::Result<(u16, Vec<u8>)> {
    use std::io::{Read, Write};

    let mut stream = std::net::TcpStream::connect(addr)?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed HTTP response"))?;
    let status = String::from_utf8_lossy(&raw[..split])
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    Ok((status, raw[split..].to_vec()))
}

/// Serializes typed params and fills `identity` from `--identity` when the
/// params don't set it.
fn params_for<M: Method>(method: &M, identity: Option<&str>) -> Value {
    let mut params = serde_json::to_value(method).unwrap_or_else(|_| json!({}));
    if let (Some(identity), Some(object)) = (identity, params.as_object_mut()) {
        object
            .entry("identity")
            .or_insert(Value::String(identity.into()));
    }
    params
}

/// A request for `method`, with `--identity` applied.
fn request_for<M: Method>(method: &M, identity: Option<&str>) -> Request {
    let mut request = Request::call("cli-1", method);
    request.params = params_for(method, identity);
    request
}

/// One RPC round trip with typed params, `--identity`, and the
/// `--stdin-json` escape hatch (replaces the params with stdin's JSON).
fn call<M: Method>(
    socket: &str,
    method: &M,
    identity: Option<&str>,
    stdin_json: bool,
) -> io::Result<Response> {
    let mut request = request_for(method, identity);
    if stdin_json {
        request.params = serde_json::from_reader(std::io::stdin()).map_err(io::Error::other)?;
        if let (Some(identity), Some(object)) = (identity, request.params.as_object_mut()) {
            object
                .entry("identity")
                .or_insert(Value::String(identity.into()));
        }
    }
    let mut client = connect_or_start_daemon(socket)?;
    let response = read_json(
        &mut client,
        &encode_json(&request).map_err(io::Error::other)?,
    )?;
    if let ResponseBody::Failure { error, .. } = &response.body {
        if error.code == ErrorCode::UnknownMethod {
            eprintln!(
                "hint: `{}` is unknown to the running daemon, which may be older than \
                 this CLI; run `idfon shutdown` and retry",
                request.method
            );
        }
    }
    Ok(response)
}

/// `call` without `--stdin-json`, for internal steps of a command.
fn call_plain<M: Method>(
    socket: &str,
    method: &M,
    identity: Option<&str>,
) -> io::Result<Response> {
    call(socket, method, identity, false)
}

/// Builds a delivery policy from the `--delivery`, `--endpoint-id` and
/// `--device-class` flags (clap restricts `mode` to the three values).
fn delivery_policy(
    mode: &str,
    endpoint_ids: Vec<String>,
    device_class: Option<String>,
) -> DeliveryPolicy {
    DeliveryPolicy {
        mode: match mode {
            "one" => DeliveryMode::One,
            "all" => DeliveryMode::All,
            _ => DeliveryMode::Failover,
        },
        endpoint_ids,
        device_class,
    }
}

/// `--out`, or a fresh temp file `idfon-<tag>-<nanos>.<ext>`.
fn out_path(out: Option<&String>, tag: &str, ext: &str) -> String {
    match out {
        Some(path) => path.clone(),
        None => std::env::temp_dir()
            .join(format!(
                "idfon-{tag}-{}.{ext}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock before epoch")
                    .as_nanos()
            ))
            .display()
            .to_string(),
    }
}

fn cmd_doctor(
    socket: &str,
    identity: Option<&str>,
    json: bool,
    stdin_json: bool,
) -> io::Result<()> {
    let status = call(socket, &rpc::DaemonStatus {}, identity, stdin_json)?;
    let peers = call(socket, &rpc::PeerList {}, identity, stdin_json)?;
    let status_result = match &status.body {
        ResponseBody::Success { result, .. } => result,
        _ => return finish(status, json),
    };
    let peers_result = match &peers.body {
        ResponseBody::Success { result, .. } => result,
        _ => return finish(peers, json),
    };
    let local = status_result
        .get("identity")
        .and_then(|v| v.get("endpoint_id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut checks = Vec::new();
    for peer in peers_result
        .get("peers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = peer.get("name").and_then(Value::as_str).unwrap_or("?");
        let id = peer.get("id").and_then(Value::as_str).unwrap_or("");
        let endpoint = peer
            .get("endpoint_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        checks.push(json!({"name": name, "peer_id": id, "endpoint_id": endpoint, "matches_peer_id": !id.is_empty() && id == endpoint}));
    }
    let result = json!({"local_endpoint_id": local, "peers": checks});
    if json {
        println!(
            "{}",
            serde_json::to_string(&json!({"ok": true, "result": result}))
                .map_err(io::Error::other)?
        );
    } else {
        println!(
            "local endpoint: {}",
            if local.is_empty() { "(missing)" } else { local }
        );
        for check in &checks {
            println!(
                "channel {}: endpoint={} peer_id={} {}",
                check["name"],
                check["endpoint_id"],
                check["peer_id"],
                if check["matches_peer_id"] == true {
                    "OK"
                } else {
                    "MISMATCH"
                }
            );
        }
    }
    Ok(())
}

fn cmd_events(
    socket: &str,
    follow: bool,
    after: Option<String>,
    event_type: Option<String>,
    json: bool,
    identity: Option<&str>,
) -> io::Result<()> {
    let request = request_for(
        &rpc::EventList {
            follow,
            after,
            event_type,
            ..Default::default()
        },
        identity,
    );
    let mut client = connect_or_start_daemon(socket)?;
    let payload = encode_json(&request).map_err(io::Error::other)?;
    let mut response = read_json(&mut client, &payload)?;
    if follow {
        loop {
            print_events(&response)?;
            response = client
                .next_response()
                .map_err(io::Error::other)?
                .json()
                .map_err(io::Error::other)?;
        }
    }
    finish(response, json)
}

fn read_json(client: &mut Client, payload: &[u8]) -> io::Result<Response> {
    client
        .request(payload)
        .map_err(io::Error::other)?
        .json()
        .map_err(io::Error::other)
}

fn print_events(response: &Response) -> io::Result<()> {
    if let idfon_protocol::ResponseBody::Success { result, .. } = &response.body {
        if let Some(events) = result.get("events").and_then(Value::as_array) {
            for event in events {
                println!(
                    "{}",
                    serde_json::to_string(event).map_err(io::Error::other)?
                );
            }
        }
    }
    Ok(())
}

fn print_human(response: &Response) {
    match &response.body {
        idfon_protocol::ResponseBody::Success { result, .. } => {
            if response.id == "cli-1" {
                if let Some(ready) = result.get("ready").and_then(serde_json::Value::as_bool) {
                    let identity = result
                        .get("identity")
                        .and_then(|identity| identity.get("name"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("none");
                    println!(
                        "{}: {}, identity: {}",
                        response.id,
                        if ready { "ready" } else { "not ready" },
                        identity
                    );
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(result).unwrap_or_default()
                    );
                }
            }
        }
        idfon_protocol::ResponseBody::Failure { error, .. } => {
            eprintln!("{:?}: {}", error.code, error.message);
        }
    }
}

/// Chunk size for CLI data transfer: bytes per `media.resource.put`/`fetch`
/// call. The JSON array encoding expands ~3-4x, so this stays safely inside
/// the 1 MB IPC frame limit.
const CHUNK: usize = 200_000;

fn generated_resource_id() -> String {
    format!(
        "data-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos()
    )
}

/// On macOS with Idfon.app installed, the app hosts the default profile's
/// daemon in-process (one endpoint per device: calls and blobs ride the
/// identity endpoints). Start it headless (no window, no Dock icon; it idles
/// out like idfond) and connect. `None` falls back to spawning idfond: not
/// macOS, a custom socket or relay flags (the app uses its own settings), the
/// app not installed, `IDFON_NO_APP=1`, or the app not serving in time.
fn start_mac_app(socket: &str) -> Option<Client> {
    if !cfg!(target_os = "macos")
        || idfon_config::get("IDFON_NO_APP").is_some_and(|value| value == "1")
        || RELAY_ARGS.get().is_some_and(|args| !args.is_empty())
    {
        return None;
    }
    let profile = PROFILE.get().cloned().flatten();
    if profile.is_some() || Path::new(socket) != idfon_config::paths::socket(None) {
        return None;
    }
    let launched = StdCommand::new("/usr/bin/open")
        .args(["-g", "-j", "-b", "com.idfon.mac", "--args", "-headless"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !launched {
        return None;
    }
    // App launch plus the daemon's store open: slower than a bare idfond.
    Client::connect_with_retry(socket, Duration::from_secs(20)).ok()
}

/// Connects to the daemon, auto-starting idfond if nothing is listening.
/// The daemon is left running after the CLI exits (dockerd-style shared
/// daemon, not one daemon per command), but shuts itself down after
/// DAEMON_IDLE_EXIT_SECS with no connected clients; the next command
/// restarts it. IDFON_IDLE_EXIT_SECS=0 disables the idle exit.
fn connect_or_start_daemon(socket: &str) -> io::Result<Client> {
    match Client::connect(socket) {
        Ok(client) => Ok(client),
        Err(ClientError::Connect(_)) => {
            if let Some(client) = start_mac_app(socket) {
                return Ok(client);
            }
            match find_daemon_binary() {
                Some(daemon) => {
                    let run_dir = Path::new(socket)
                        .parent()
                        .map(|dir| dir.to_path_buf())
                        .unwrap_or_default();
                    let _ = std::fs::create_dir_all(&run_dir);
                    // The profile's own socket: idfond picks the profile's
                    // durable data dir (the one the apps use too). A custom
                    // --socket keeps its data next to it, as before.
                    let profile = PROFILE.get().cloned().flatten();
                    let profile_socket = idfon_config::paths::socket(profile.as_deref());
                    let location: Vec<String> = if Path::new(socket) == profile_socket {
                        match profile {
                            Some(name) => vec!["--profile".into(), name],
                            None => vec![],
                        }
                    } else {
                        vec![
                            "--socket".into(),
                            socket.into(),
                            "--data-dir".into(),
                            run_dir.to_string_lossy().into_owned(),
                        ]
                    };
                    // Capture the daemon's stderr (its eprintln diagnostics and
                    // the shared subscriber's stderr sink) so a CLI-spawned
                    // daemon is not a logging black hole.
                    let daemon_log = run_dir.join("idfond.log");
                    // Two attempts: a just-killed daemon may still be dying
                    // (lock/socket still held), so the first spawn can lose.
                    let mut last_error = None;
                    for _ in 0..2 {
                        let stderr = std::fs::File::create(&daemon_log)
                            .map(Stdio::from)
                            .unwrap_or_else(|_| Stdio::null());
                        let spawned = StdCommand::new(&daemon)
                            .args(&location)
                            .args(RELAY_ARGS.get().map_or(&[][..], Vec::as_slice))
                            .env(
                                "IDFON_IDLE_EXIT_SECS",
                                if *KEEP_ALIVE.get().unwrap_or(&false) {
                                    "0".to_string()
                                } else {
                                    idfon_config::get("IDFON_IDLE_EXIT_SECS")
                                        .unwrap_or_else(|| DAEMON_IDLE_EXIT_SECS.into())
                                },
                            )
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(stderr)
                            .spawn();
                        match spawned {
                            Err(error) => {
                                return Err(io::Error::other(format!(
                                    "failed to start daemon at {}: {error}",
                                    daemon.display()
                                )));
                            }
                            Ok(_) => match Client::connect_with_retry(socket, DAEMON_START_TIMEOUT)
                            {
                                Ok(client) => return Ok(client),
                                Err(error) => last_error = Some(error),
                            },
                        }
                    }
                    Err(io::Error::other(format!(
                        "daemon unavailable; tried to start idfond automatically: {}",
                        last_error.expect("at least one connect attempt")
                    )))
                }
                None => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "daemon unavailable and idfond not found (install it or add it to PATH)",
                )),
            }
        }
        other => other.map_err(|error| io::Error::other(error.to_string())),
    }
}

/// Infers a mime type from the file extension for common audio/video types.
fn guess_mime(path: &str) -> Option<String> {
    let ext = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    Some(
        match ext.as_str() {
            "mp4" | "m4v" | "cmfv" => "video/mp4",
            "webm" => "video/webm",
            "mkv" => "video/x-matroska",
            "mov" => "video/quicktime",
            "ts" | "m2ts" | "mts" => "video/mp2t",
            "avi" => "video/x-msvideo",
            "opus" => "audio/opus",
            "ogg" => "audio/ogg",
            "wav" => "audio/wav",
            "mp3" => "audio/mpeg",
            _ => return None,
        }
        .to_string(),
    )
}

fn response_bytes(result: &Value) -> io::Result<Vec<u8>> {
    result
        .get("bytes")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_u64().unwrap_or(0) as u8)
                .collect::<Vec<u8>>()
        })
        .ok_or_else(|| io::Error::other("response missing bytes"))
}

/// Reads stdin/FILE and stores it with the daemon (chunked when larger than
/// one frame). Returns the final daemon response (finish/one-shot), whose
/// result carries `blob_ticket`, `size_bytes`, and `content_hash`.
fn put_data(
    socket: &str,
    file: Option<&String>,
    resource_id: Option<String>,
    mime: Option<&String>,
    identity: Option<&str>,
) -> io::Result<Response> {
    let data = match file {
        Some(path) => std::fs::read(path).map_err(|error| {
            io::Error::new(error.kind(), format!("cannot read {path}: {error}"))
        })?,
        None => {
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut std::io::stdin(), &mut data)?;
            data
        }
    };
    let resource_id = resource_id.unwrap_or_else(generated_resource_id);
    let mime = mime
        .cloned()
        .or_else(|| file.and_then(|path| guess_mime(path)));
    let put = |chunk: &[u8], append: bool, finish: bool| {
        let params = rpc::BlobPut {
            resource_id: resource_id.clone(),
            bytes: chunk.to_vec(),
            append,
            finish,
            mime: mime.clone(),
        };
        call_plain(socket, &params, identity)
    };
    let mut response = None;
    let mut offset = 0;
    while offset < data.len() || response.is_none() {
        let end = (offset + CHUNK).min(data.len());
        response = Some(put(&data[offset..end], offset > 0, false)?);
        offset = end;
        if offset >= data.len() {
            break;
        }
    }
    if offset > CHUNK {
        response = Some(put(&[], false, true)?);
    }
    let response = response.expect("at least one request");
    if !response.ok {
        return Err(io::Error::other(request_error(&response)));
    }
    Ok(response)
}

fn result_str(response: &Response, key: &str) -> String {
    match &response.body {
        ResponseBody::Success { result, .. } => match result.get(key) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Number(number)) => number.to_string(),
            Some(Value::Bool(flag)) => flag.to_string(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

fn result_usize(response: &Response, key: &str) -> usize {
    match &response.body {
        ResponseBody::Success { result, .. } => result
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize,
        _ => 0,
    }
}

fn request_error(response: &Response) -> String {
    match &response.body {
        ResponseBody::Failure { error, .. } => format!("{:?}: {}", error.code, error.message),
        _ => "request failed".into(),
    }
}

fn cmd_put(
    socket: &str,
    file: Option<&String>,
    resource_id: Option<String>,
    mime: Option<&String>,
    json: bool,
    identity: Option<&str>,
) -> io::Result<()> {
    let response = put_data(socket, file, resource_id, mime, identity)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&response).map_err(io::Error::other)?
        );
        return Ok(());
    }
    println!("{}", result_str(&response, "blob_ticket"));
    Ok(())
}

/// `idfon get TICKET`: streams the blob to stdout (or --out FILE) in chunks.
fn cmd_get(
    socket: &str,
    ticket: &str,
    out: Option<&String>,
    identity: Option<&str>,
) -> io::Result<()> {
    fetch_to(socket, ticket, out, identity).map(|_| ())
}

/// Streams the blob to stdout or a file in slices; returns bytes written.
fn fetch_to(
    socket: &str,
    ticket: &str,
    out: Option<&String>,
    identity: Option<&str>,
) -> io::Result<usize> {
    let resource_id = generated_resource_id();
    let mut writer: Box<dyn std::io::Write> = match out {
        Some(path) => Box::new(std::io::BufWriter::new(std::fs::File::create(path)?)),
        None => Box::new(std::io::stdout().lock()),
    };
    let mut offset = 0usize;
    loop {
        let params = rpc::BlobGet {
            resource_id: resource_id.clone(),
            blob_ticket: ticket.to_owned(),
            offset: Some(offset as u64),
            length: Some(CHUNK as u64),
        };
        let response = call_plain(socket, &params, identity)?;
        if !response.ok {
            return Err(io::Error::other(request_error(&response)));
        }
        let (bytes, total_size) = match &response.body {
            ResponseBody::Success { result, .. } => (
                response_bytes(result)?,
                result
                    .get("total_size")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as usize,
            ),
            _ => return Err(io::Error::other("unexpected response")),
        };
        writer.write_all(&bytes)?;
        offset += bytes.len();
        if offset >= total_size {
            break;
        }
        if bytes.is_empty() {
            return Err(io::Error::other("fetch stalled: empty chunk"));
        }
    }
    writer.flush()?;
    Ok(offset)
}

/// `idfon send-data PEER`: stores stdin/FILE as a blob, then signals the
/// ticket to the peer through the message path (IDFON-DATA/1 envelope).
/// Prints the bare BlobTicket once delivery is acknowledged.
/// Text and signaled-blob modes of `send` (PEER required).
fn send_payload(
    socket: &str,
    stdin_json: bool,
    args: &SendArgs,
    peer: &str,
    json: bool,
    identity: Option<&str>,
) -> io::Result<()> {
    if let Some(file) = &args.file {
        let file = if file == "-" { None } else { Some(file) };
        cmd_send_data(socket, peer, file, args.retries, json, identity)
    } else {
        // Per-invocation key by default: unique across the daemon's
        // persisted operations, so retries (handled within one
        // operation) stay exactly-once. Pass --idempotency-key only
        // for deliberate cross-invocation dedup of the same message.
        let key = args.idempotency_key.clone().unwrap_or_else(|| {
            format!(
                "idfon-text-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock before epoch")
                    .as_nanos()
            )
        });
        let Some(text) = args.message.clone().or_else(|| args.text.clone()) else {
            return Err(io::Error::other(
                "nothing to send: give a MESSAGE (`idfon send bob \"hi\"`) or --file FILE",
            ));
        };
        // Resolve and take the reply baseline before sending: a fast reply
        // must not land before the cursor we wait from.
        let reply_from = if args.wait {
            Some((
                ux::resolve_peer(socket, identity, peer)?,
                ux::latest_cursor(socket, identity),
            ))
        } else {
            None
        };
        let params = rpc::MessageSend {
            peer: peer.to_owned(),
            text: Some(text),
            idempotency_key: key,
            conversation: args.conversation.clone(),
            context: args.context.clone(),
            capability_ticket: args.capability_ticket.clone(),
            retries: args.retries,
            delivery: Some(delivery_policy(
                &args.delivery,
                args.endpoint_ids.clone(),
                args.device_class.clone(),
            )),
        };
        let response = call(socket, &params, identity, stdin_json)?;
        let Some((target, baseline)) = reply_from else {
            if json || !response.ok {
                return finish(response, json);
            }
            println!("sent to {peer} ({})", result_str(&response, "operation_id"));
            return Ok(());
        };
        if !response.ok {
            return finish(response, json);
        }
        let reply = ux::wait_message(socket, identity, baseline, args.timeout, |event| {
            let data = &event["data"];
            ux::peer_matches(&target, data["peer_id"].as_str().unwrap_or(""))
                && data["conversation"].is_null()
                && !data["text"].as_str().unwrap_or("").starts_with("IDFON-")
        })?;
        match reply {
            Some(event) if json => println!("{event}"),
            Some(event) => println!(
                "{}: {}",
                target.name,
                event["data"]["text"].as_str().unwrap_or_default()
            ),
            None => {
                return Err(io::Error::other(format!(
                    "sent, but no reply from {peer} within {}s",
                    args.timeout.as_secs()
                )))
            }
        }
        Ok(())
    }
}

fn cmd_send_data(
    socket: &str,
    peer: &str,
    file: Option<&String>,
    retries: Option<u64>,
    json: bool,
    identity: Option<&str>,
) -> io::Result<()> {
    let response = put_data(socket, file, None, None, identity)?;
    let ticket = result_str(&response, "blob_ticket");
    let size = result_usize(&response, "size_bytes");
    let text = format!("IDFON-DATA/1\nticket={ticket}\nsize={size}");
    // The ticket is content-addressed, so this key is stable across retries
    // with the same content and unique across different content.
    let idempotency_key = format!("idfon-data-{ticket}");
    let params = rpc::MessageSend {
        peer: peer.to_owned(),
        text: Some(text),
        idempotency_key,
        retries,
        ..Default::default()
    };
    let response = call_plain(socket, &params, identity)?;
    if !response.ok {
        return Err(io::Error::other(request_error(&response)));
    }
    let operation_id = result_str(&response, "operation_id");
    loop {
        let params = rpc::OperationWait {
            operation_id: operation_id.clone(),
            timeout_ms: None,
        };
        let response = call_plain(socket, &params, identity)?;
        let status = match &response.body {
            ResponseBody::Success { result, .. } => result
                .get("operation")
                .and_then(|operation| operation.get("status"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        };
        match status.as_str() {
            "delivered" => break,
            "failed" | "expired" | "cancelled" => {
                return Err(io::Error::other(format!("send-data {status}")));
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    }
    if json {
        println!(
            "{{\"blob_ticket\":\"{ticket}\",\"operation_id\":\"{operation_id}\",\"status\":\"delivered\"}}"
        );
    } else {
        println!(
            "sent {} to {peer} ({size} bytes, delivered)",
            file.map_or("stdin", String::as_str)
        );
    }
    Ok(())
}

/// `idfon recv`: saves a file a peer sent (an `IDFON-DATA/1` message). Takes
/// the newest one that arrived within `since`, else waits for the next.
fn cmd_recv(
    socket: &str,
    out: Option<&String>,
    from: Option<&str>,
    since: Duration,
    timeout: Duration,
    identity: Option<&str>,
) -> io::Result<()> {
    let from = from
        .map(|reference| ux::resolve_peer(socket, identity, reference))
        .transpose()?;
    let is_file = |event: &Value| {
        let data = &event["data"];
        from.as_ref()
            .is_none_or(|peer| ux::peer_matches(peer, data["peer_id"].as_str().unwrap_or("")))
            && ux::data_envelope(data["text"].as_str().unwrap_or("")).is_some()
    };
    let cutoff = ux::unix_now().saturating_sub(since.as_secs());
    let timestamp = |event: &Value| match &event["timestamp"] {
        Value::String(text) => text.parse().unwrap_or(0),
        other => other.as_u64().unwrap_or(0),
    };
    let recent = ux::message_events(socket, identity, None)?
        .into_iter()
        .rev()
        .find(|event| is_file(event) && timestamp(event) >= cutoff);
    let event = match recent {
        Some(event) => event,
        None => {
            let after = ux::latest_cursor(socket, identity);
            eprintln!("waiting up to {}s for a file...", timeout.as_secs());
            ux::wait_message(socket, identity, after, timeout, is_file)?.ok_or_else(|| {
                io::Error::other(format!(
                    "no file arrived within {}s{}",
                    timeout.as_secs(),
                    from.as_ref()
                        .map(|peer| format!(" from {}", peer.name))
                        .unwrap_or_default()
                ))
            })?
        }
    };
    let data = &event["data"];
    let (ticket, declared_size) =
        ux::data_envelope(data["text"].as_str().unwrap_or("")).expect("filtered to files");
    let written = fetch_to(socket, &ticket, out, identity)?;
    if let Some(size) = declared_size {
        if written != size {
            return Err(io::Error::other(format!(
                "received {written} bytes but the sender declared {size}"
            )));
        }
    }
    let peers = ux::all_peers(socket, identity);
    eprintln!(
        "saved {written} bytes from {}{}",
        ux::sender_name(&peers, data["peer_id"].as_str().unwrap_or("")),
        out.map(|path| format!(" to {path}")).unwrap_or_default()
    );
    Ok(())
}

/// `idfon stream`: publishes FILE (or stdin, spooled to a temp file) as a
/// live iroh broadcast through the daemon. Prints the bare live ticket.
/// Resolves --file to a daemon-readable path; stdin (None or "-") is spooled
/// to a temp file because the daemon opens the path itself. Returns the path
/// string and the temp path to keep alive for the call.
fn spool_stdin() -> io::Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "idfon-stream-{}.wav",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos()
    ));
    let mut data = Vec::new();
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut data)?;
    std::fs::write(&path, data)?;
    Ok(path)
}

/// Resolves --file to a daemon-readable path; stdin (None or "-") is spooled
/// to a temp file because the daemon opens the path itself.
fn resolve_stream_file(file: Option<&String>) -> io::Result<String> {
    match file {
        None => {
            let path = spool_stdin()?;
            path.to_str()
                .map(str::to_string)
                .ok_or_else(|| io::Error::other("temp path not utf-8"))
        }
        Some(path) if path == "-" => {
            let path = spool_stdin()?;
            path.to_str()
                .map(str::to_string)
                .ok_or_else(|| io::Error::other("temp path not utf-8"))
        }
        Some(path) => ux::daemon_path(path),
    }
}

/// `idfon listen TICKET`: records a remote live broadcast for --seconds
/// (default 15) into --out (default: a temp file copied to stdout). With
/// --video, records the selected rendition's encoded H.264 (Annex B).
#[allow(clippy::too_many_arguments)]
fn cmd_listen(
    socket: &str,
    ticket: &str,
    out: Option<&String>,
    seconds: Option<u64>,
    video: bool,
    quality: Option<&str>,
    relay: bool,
    json: bool,
    identity: Option<&str>,
) -> io::Result<()> {
    let ext = if video { "h264" } else { "wav" };
    let params = rpc::LiveSubscribe {
        ticket: ticket.to_owned(),
        out: out_path(out, "listen", ext),
        video,
        relay,
        quality: quality.map(str::to_owned),
        seconds,
    };
    let response = call_plain(socket, &params, identity)?;
    if !response.ok {
        return Err(io::Error::other(request_error(&response)));
    }
    let written_path = match &response.body {
        ResponseBody::Success { result, .. } => result
            .get("out")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => return Err(io::Error::other("unexpected response")),
    };
    if out.is_none() {
        std::io::copy(
            &mut std::fs::File::open(&written_path)?,
            &mut std::io::stdout().lock(),
        )?;
        let _ = std::fs::remove_file(&written_path);
    }
    if json {
        println!(
            "{}",
            serde_json::to_string(&response).map_err(io::Error::other)?
        );
    } else if video {
        eprintln!(
            "listen: wrote {} ({} frames, {} ms)",
            written_path,
            result_str(&response, "frames"),
            result_str(&response, "duration_ms"),
        );
    } else {
        eprintln!(
            "listen: wrote {} ({})",
            written_path,
            result_str(&response, "duration_ms") + " ms audio"
        );
    }
    Ok(())
}

/// `idfon answer`: waits for an inbound 1:1 call and records it to --out
/// (default: a temp file copied to stdout). With --video, records the
/// selected rendition's encoded H.264 (Annex B).
fn cmd_answer(
    socket: &str,
    out: Option<&String>,
    seconds: Option<u64>,
    wait: Option<u64>,
    from: Option<&str>,
    video: bool,
    quality: Option<&str>,
    relay: bool,
    json: bool,
    identity: Option<&str>,
) -> io::Result<()> {
    let ext = if video { "h264" } else { "wav" };
    let params = rpc::LiveAnswer {
        peer: from.map(str::to_owned),
        out: out_path(out, "answer", ext),
        video,
        relay,
        quality: quality.map(str::to_owned),
        seconds,
        wait,
    };
    let response = call_plain(socket, &params, identity)?;
    if !response.ok {
        return Err(io::Error::other(request_error(&response)));
    }
    let written_path = match &response.body {
        ResponseBody::Success { result, .. } => result
            .get("out")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => return Err(io::Error::other("unexpected response")),
    };
    if out.is_none() {
        std::io::copy(
            &mut std::fs::File::open(&written_path)?,
            &mut std::io::stdout().lock(),
        )?;
        let _ = std::fs::remove_file(&written_path);
    }
    if json {
        println!(
            "{}",
            serde_json::to_string(&response).map_err(io::Error::other)?
        );
    } else {
        eprintln!(
            "answer: wrote {} ({})",
            written_path,
            result_str(&response, "duration_ms") + " ms audio"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_idfon_resource_urls() {
        assert_eq!(
            split_idfon_url("idfon://acct/fs/a.txt").unwrap(),
            ("acct".to_string(), "/fs/a.txt".to_string())
        );
        assert_eq!(
            split_idfon_url("idfon://acct").unwrap(),
            ("acct".to_string(), "/".to_string())
        );
        assert!(split_idfon_url("https://x/y").is_err());
        assert!(split_idfon_url("idfon:///x").is_err());
    }

    #[test]
    fn http_get_parses_status_and_body() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                )
                .unwrap();
        });
        let (status, body) = http_get(&addr, "/x", "tok").unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"hello");
        server.join().unwrap();
    }

    #[test]
    fn parses_rental_durations() {
        assert_eq!(parse_duration("60"), Ok(60));
        assert_eq!(parse_duration("45m"), Ok(2_700));
        assert_eq!(parse_duration("12h"), Ok(43_200));
        assert_eq!(parse_duration("30d"), Ok(2_592_000));
        assert!(parse_duration("0d").is_err());
        assert!(parse_duration("30y").is_err());
        assert!(parse_duration("d").is_err());
    }
}
