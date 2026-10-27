//! Sends: copies of a track's signal into folders and buses (return tracks), next to its
//! main output.

use super::{check_send, check_track, error};
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::project::{SendId, TrackId, TrackSend, SEND_GAIN_RANGE};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(super) fn register(r: &mut Registry) {
    r.add(
        "add_send",
        "Send a copy of `track`'s signal into `target`, a folder or bus track (add a bus with \
         add_track kind \"bus\" for a reverb return), alongside its main output. `gain_db` is \
         the send level (-60 to +12 dB, default 0). `pre_fader` takes the signal before the \
         track's fader and pan, so the fader doesn't change it (default false: post-fader). \
         Refused if it would create a loop. Returns the send with its id.",
        Safety::Edit,
        add_send,
    );
    r.add(
        "add_send_to_new_bus",
        "Add a new bus (a return track, at the end of the project, routed to the master) and \
         send `track` into it: post-fader at 0 dB, or as `gain_db` and `pre_fader` say. Put a \
         reverb or delay on the bus with add_insert. Returns the send, whose `target` is the \
         new bus's track id.",
        Safety::Edit,
        add_send_to_new_bus,
    );
    r.add(
        "set_send",
        "Change a send by its id: `gain_db` (-60 to +12 dB), `pre_fader`, `mute`, or `target` \
         (another folder or bus; refused if it would create a loop). Only the fields given \
         change. Muting the track also silences its sends, pre-fader ones included.",
        Safety::Edit,
        set_send,
    );
    r.add(
        "remove_send",
        "Delete a send by its id.",
        Safety::Destructive,
        |host, input: SendArg| {
            let s = host.session();
            check_send(s, input.send)?;
            s.project.remove_send(input.send);
            s.mark_dirty();
            Ok(Done { done: true })
        },
    );
}

#[derive(Serialize, JsonSchema)]
struct Done {
    done: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SendArg {
    send: SendId,
}

/// A send as actions report it.
#[derive(Serialize, JsonSchema)]
pub(super) struct SendView {
    send: SendId,
    /// The track it belongs to.
    track: TrackId,
    target: TrackId,
    gain_db: f32,
    pre_fader: bool,
    mute: bool,
}

impl SendView {
    pub(super) fn new(track: TrackId, send: &TrackSend) -> Self {
        Self {
            send: send.id,
            track,
            target: send.target,
            gain_db: send.gain_db,
            pre_fader: send.pre_fader,
            mute: send.mute,
        }
    }
}

fn check_gain(gain_db: Option<f32>) -> ApiResult<()> {
    let (low, high) = SEND_GAIN_RANGE;
    match gain_db {
        Some(g) if !(low..=high).contains(&g) => {
            Err(error(format!("gain_db {g} is outside {low}..{high}")))
        }
        _ => Ok(()),
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddSend {
    track: TrackId,
    /// The folder or bus track to send into.
    target: TrackId,
    /// Send level in dB, -60 to +12; 0 when omitted.
    #[serde(default)]
    gain_db: Option<f32>,
    /// Take the signal before the fader and pan.
    #[serde(default)]
    pre_fader: bool,
}

fn add_send(host: &mut dyn Host, input: AddSend) -> ApiResult<SendView> {
    let s = host.session();
    check_track(s, input.track)?;
    check_gain(input.gain_db)?;
    let id = s
        .project
        .add_send(input.track, input.target, input.pre_fader)
        .map_err(error)?;
    let send = s.project.find_send_mut(id).expect("just added");
    if let Some(gain) = input.gain_db {
        send.gain_db = gain;
    }
    let send = *send;
    s.mark_dirty();
    Ok(SendView::new(input.track, &send))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddSendToNewBus {
    track: TrackId,
    /// Send level in dB, -60 to +12; 0 when omitted.
    #[serde(default)]
    gain_db: Option<f32>,
    /// Take the signal before the fader and pan.
    #[serde(default)]
    pre_fader: bool,
}

fn add_send_to_new_bus(host: &mut dyn Host, input: AddSendToNewBus) -> ApiResult<SendView> {
    let s = host.session();
    check_track(s, input.track)?;
    check_gain(input.gain_db)?;
    let (_, id) = s.add_send_to_new_bus(input.track)?;
    let send = s.project.find_send_mut(id).expect("just added");
    if let Some(gain) = input.gain_db {
        send.gain_db = gain;
    }
    send.pre_fader = input.pre_fader;
    let send = *send;
    Ok(SendView::new(input.track, &send))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetSend {
    send: SendId,
    /// Send level in dB, -60 to +12.
    #[serde(default)]
    gain_db: Option<f32>,
    #[serde(default)]
    pre_fader: Option<bool>,
    #[serde(default)]
    mute: Option<bool>,
    /// Another folder or bus track to send into.
    #[serde(default)]
    target: Option<TrackId>,
}

fn set_send(host: &mut dyn Host, input: SetSend) -> ApiResult<SendView> {
    let s = host.session();
    check_send(s, input.send)?;
    check_gain(input.gain_db)?;
    // The target first: it is the only change that can be refused.
    if let Some(target) = input.target {
        s.project
            .set_send_target(input.send, target)
            .map_err(error)?;
    }
    let structural = input.target.is_some() || input.pre_fader.is_some();
    let send = s.project.find_send_mut(input.send).expect("checked");
    if let Some(gain) = input.gain_db {
        send.gain_db = gain;
    }
    if let Some(pre) = input.pre_fader {
        send.pre_fader = pre;
    }
    if let Some(mute) = input.mute {
        send.mute = mute;
    }
    // Level and mute reach the engine without a rebuild; the target and tap need one.
    if structural {
        s.mark_dirty();
    }
    let (track, send) = s.project.find_send(input.send).expect("checked");
    Ok(SendView::new(track, send))
}
