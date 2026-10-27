//! MIDI keyboards and controllers through midir.

use midir::{MidiInput, MidiInputConnection};
use rtrb::{Consumer, Producer, RingBuffer};
use std::time::Instant;
use vst3_host::MidiEvent;

/// A MIDI event and when it came in. Stamped on arrival with the same clock the engine
/// reads at the start of each device callback, so the engine can place it on its sample.
/// (midir's own stamp counts from when the port opened, in milliseconds on Windows, and
/// can't be compared with the audio clock.)
#[derive(Debug, Clone, Copy)]
pub struct TimedMidi {
    pub event: MidiEvent,
    pub at: Instant,
}

pub fn port_names() -> Vec<String> {
    let Ok(input) = MidiInput::new("daw-scan") else {
        return Vec::new();
    };
    input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect()
}

/// Open MIDI inputs. Each port feeds its own lock-free ring, drained by the audio thread.
#[derive(Default)]
pub struct MidiInputs {
    connections: Vec<MidiInputConnection<Producer<TimedMidi>>>,
    /// Names of the open ports; a port's index here is its index in the engine.
    pub ports: Vec<String>,
    pub errors: Vec<String>,
}

impl MidiInputs {
    /// Open every available input port. Returns the consumers for the engine, in port order.
    ///
    /// On Windows a MIDI port can be open in only one application at a time, so a port another
    /// DAW holds shows up in [`Self::errors`].
    pub fn open_all() -> (Self, Vec<Consumer<TimedMidi>>) {
        let mut inputs = Self::default();
        let mut consumers = Vec::new();
        let Ok(scan) = MidiInput::new("daw-scan") else {
            inputs.errors.push("MIDI is unavailable".into());
            return (inputs, consumers);
        };
        for port in scan.ports() {
            let name = scan.port_name(&port).unwrap_or_else(|_| "MIDI port".into());
            let Ok(input) = MidiInput::new("daw") else {
                continue;
            };
            let (tx, rx) = RingBuffer::new(1024);
            let result = input.connect(
                &port,
                "daw-in",
                |_stamp, bytes, tx: &mut Producer<TimedMidi>| {
                    let at = Instant::now();
                    if let Some(event) = MidiEvent::from_midi_bytes(bytes) {
                        let _ = tx.push(TimedMidi { event, at });
                    }
                },
                tx,
            );
            match result {
                Ok(connection) => {
                    inputs.connections.push(connection);
                    inputs.ports.push(name);
                    consumers.push(rx);
                }
                Err(e) => inputs.errors.push(format!("{name}: {e}")),
            }
        }
        (inputs, consumers)
    }

    pub fn port_index(&self, name: &str) -> Option<usize> {
        self.ports.iter().position(|p| p == name)
    }
}
