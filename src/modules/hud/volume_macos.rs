//! Default output volume via CoreAudio property listeners: no polling. Listeners run on a
//! CoreAudio thread; `on_change` must be Send.

use std::ffi::c_void;
use std::sync::Mutex;

type ObjectId = u32;
type OsStatus = i32;

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

type Listener = unsafe extern "C" fn(ObjectId, u32, *const Address, *mut c_void) -> OsStatus;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        id: ObjectId,
        addr: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectHasProperty(id: ObjectId, addr: *const Address) -> u8;
    fn AudioObjectAddPropertyListener(
        id: ObjectId,
        addr: *const Address,
        listener: Listener,
        client: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectRemovePropertyListener(
        id: ObjectId,
        addr: *const Address,
        listener: Listener,
        client: *mut c_void,
    ) -> OsStatus;
}

const fn cc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}
const SYSTEM_OBJECT: ObjectId = 1;
const DEFAULT_OUTPUT: Address = Address {
    selector: cc(b"dOut"),
    scope: cc(b"glob"),
    element: 0,
};
const VOLUME: u32 = cc(b"volm");
const MUTE: u32 = cc(b"mute");
const OUTPUT: u32 = cc(b"outp");
/// Main element, then left/right channels (many devices only have per-channel volume).
const ELEMENTS: [u32; 3] = [0, 1, 2];

fn addr(selector: u32, element: u32) -> Address {
    Address {
        selector,
        scope: OUTPUT,
        element,
    }
}

fn get<T: Default>(id: ObjectId, a: &Address) -> Option<T> {
    let mut v = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: `v` is a valid out-buffer of `size` bytes.
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            a,
            0,
            std::ptr::null(),
            &mut size,
            &mut v as *mut T as *mut c_void,
        )
    };
    (st == 0).then_some(v)
}

fn has(id: ObjectId, a: &Address) -> bool {
    // SAFETY: plain query on a valid address struct.
    unsafe { AudioObjectHasProperty(id, a) != 0 }
}

fn read(device: ObjectId) -> Option<(f32, bool)> {
    let vol = if has(device, &addr(VOLUME, 0)) {
        get::<f32>(device, &addr(VOLUME, 0))?
    } else {
        let ch: Vec<f32> = [1, 2]
            .iter()
            .filter_map(|&e| get::<f32>(device, &addr(VOLUME, e)))
            .collect();
        if ch.is_empty() {
            return None;
        }
        ch.iter().sum::<f32>() / ch.len() as f32
    };
    let muted = get::<u32>(device, &addr(MUTE, 0)).unwrap_or(0) != 0;
    Some((vol, muted))
}

struct State {
    on_change: Box<dyn Fn(f32, bool) + Send + Sync>,
    device: Mutex<ObjectId>,
    last: Mutex<Option<(f32, bool)>>,
}

impl State {
    fn client(&self) -> *mut c_void {
        self as *const State as *mut c_void
    }

    fn listen_device(&self, device: ObjectId, on: bool) {
        for sel in [VOLUME, MUTE] {
            for e in ELEMENTS {
                let a = addr(sel, e);
                if has(device, &a) {
                    // SAFETY: `self` outlives the listener (removed in Drop of Watcher).
                    unsafe {
                        if on {
                            AudioObjectAddPropertyListener(device, &a, on_property, self.client());
                        } else {
                            AudioObjectRemovePropertyListener(
                                device,
                                &a,
                                on_property,
                                self.client(),
                            );
                        }
                    }
                }
            }
        }
    }

    fn check(&self) {
        let device = *self.device.lock().unwrap_or_else(|e| e.into_inner());
        let Some(now) = read(device) else { return };
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        // CoreAudio fires once per channel; report each distinct value once.
        if last.is_some_and(|(v, m)| (v - now.0).abs() < 0.001 && m == now.1) {
            return;
        }
        let first = last.is_none();
        *last = Some(now);
        drop(last);
        if !first {
            (self.on_change)(now.0, now.1);
        }
    }
}

unsafe extern "C" fn on_property(
    id: ObjectId,
    _n: u32,
    _addrs: *const Address,
    client: *mut c_void,
) -> OsStatus {
    // SAFETY: `client` is the boxed State registered by `watch`, alive until removal.
    let state = unsafe { &*(client as *const State) };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if id == SYSTEM_OBJECT {
            // Default output device changed: move listeners to the new one.
            let Some(new) = get::<ObjectId>(SYSTEM_OBJECT, &DEFAULT_OUTPUT) else {
                return;
            };
            let old = std::mem::replace(
                &mut *state.device.lock().unwrap_or_else(|e| e.into_inner()),
                new,
            );
            state.listen_device(old, false);
            state.listen_device(new, true);
            *state.last.lock().unwrap_or_else(|e| e.into_inner()) = read(new);
        } else {
            state.check();
        }
    }));
    if r.is_err() {
        log::error!("volume listener panicked");
    }
    0
}

pub struct Watcher(Box<State>);

impl Drop for Watcher {
    fn drop(&mut self) {
        let device = *self.0.device.lock().unwrap_or_else(|e| e.into_inner());
        self.0.listen_device(device, false);
        // SAFETY: same listener/client pair as registered.
        unsafe {
            AudioObjectRemovePropertyListener(
                SYSTEM_OBJECT,
                &DEFAULT_OUTPUT,
                on_property,
                self.0.client(),
            );
        }
    }
}

pub fn watch(on_change: impl Fn(f32, bool) + Send + Sync + 'static) -> Option<Watcher> {
    let device = get::<ObjectId>(SYSTEM_OBJECT, &DEFAULT_OUTPUT)?;
    let state = Box::new(State {
        on_change: Box::new(on_change),
        device: Mutex::new(device),
        last: Mutex::new(read(device)),
    });
    state.listen_device(device, true);
    // SAFETY: the boxed state lives in the returned Watcher; Drop removes the listener.
    unsafe {
        AudioObjectAddPropertyListener(SYSTEM_OBJECT, &DEFAULT_OUTPUT, on_property, state.client());
    }
    Some(Watcher(state))
}
