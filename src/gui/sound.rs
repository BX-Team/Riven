use std::f32::consts::TAU;
use std::num::NonZero;

use rodio::buffer::SamplesBuffer;

const RATE: u32 = 48_000;
/// Two quick soft notes, C6 then F6, the second starting while the first still rings.
const NOTES: [(f32, f32); 2] = [(1046.5, 0.), (1396.9, 0.08)];
const LENGTH: f32 = 0.5;
/// The last stretch eases to silence; a note cut while still audible clicks.
const FADE: f32 = 0.2;
const GAIN: f32 = 0.16;

/// The launcher's short two-note chime for friend requests and invites, played off the UI thread.
/// No sound device is fine: the toast is the message.
pub fn chime() {
    std::thread::spawn(|| {
        let Ok(mut device) = rodio::DeviceSinkBuilder::open_default_sink() else {
            return;
        };
        device.log_on_drop(false);
        let (Some(channels), Some(rate)) = (NonZero::new(1), NonZero::new(RATE)) else {
            return;
        };
        let player = rodio::Player::connect_new(device.mixer());
        player.append(SamplesBuffer::new(channels, rate, samples()));
        player.sleep_until_end();
        // The device still holds its last buffer when the player is done; closing it at once cuts it off.
        std::thread::sleep(std::time::Duration::from_millis(250));
    });
}

/// A quick swell, then each partial dies away, the upper ones sooner, like a soft mallet.
fn samples() -> Vec<f32> {
    let total = (LENGTH * RATE as f32) as usize;
    (0..total)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let left = ((LENGTH - t) / FADE).clamp(0., 1.);
            let tail = 0.5 - 0.5 * (std::f32::consts::PI * left).cos();
            let ring: f32 = NOTES.iter().map(|&(hz, start)| note(hz, t - start)).sum();
            ring * GAIN * tail
        })
        .collect()
}

fn note(hz: f32, t: f32) -> f32 {
    if t < 0. {
        return 0.;
    }
    let swell = (t / 0.006).min(1.);
    [(1., 1., 0.11), (2., 0.12, 0.05), (3., 0.03, 0.03)]
        .iter()
        .map(|&(partial, level, decay)| level * (TAU * hz * partial * t).sin() * (-t / decay).exp())
        .sum::<f32>()
        * swell
}
