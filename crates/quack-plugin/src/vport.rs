//! Virtual MIDI source for the MIDI output (macOS).
//!
//! Logic Pro ignores MIDI sent by an audio effect, so on macOS each
//! instance with MIDI Out on also publishes a CoreMIDI source named
//! "Quackapella": any app, Logic's software instrument tracks included,
//! can record or play from it like from a hardware port.
//!
//! The audio thread never calls CoreMIDI (it talks to the MIDI server):
//! it pushes messages into a lock-free queue, and a sender thread
//! drains it every millisecond, which adds at most about 1 ms. The
//! source exists only while MIDI Out is on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use quack_dsp::midiout::Msg;

/// Name of the virtual source.
pub const PORT_NAME: &str = "Quackapella";

const QUEUE: usize = 1024;

/// Single-producer, single-consumer queue of packed messages.
#[derive(Debug)]
pub struct Queue {
    slots: [AtomicU32; QUEUE],
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| AtomicU32::new(0)),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }
}

impl Queue {
    /// Producer side (audio thread). Drops the message when full.
    pub fn push(&self, m: Msg) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= QUEUE {
            return false;
        }
        self.slots[head % QUEUE].store(m.pack(), Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// Consumer side (sender thread).
    pub fn pop(&self) -> Option<Msg> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        let m = Msg::unpack(self.slots[tail % QUEUE].load(Ordering::Relaxed));
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(m)
    }
}

/// State shared by the audio thread and the sender thread.
#[derive(Debug, Default)]
pub struct Link {
    pub queue: Queue,
    /// MIDI Out is on: the source should exist.
    pub enabled: AtomicBool,
    /// The instance is gone: the thread disposes the source and exits.
    pub stop: AtomicBool,
}

/// One instance's virtual port: the audio side of it.
#[derive(Debug, Default)]
pub struct VirtualPort {
    link: Option<Arc<Link>>,
}

impl VirtualPort {
    /// Start the sender thread (not on the audio thread). Once.
    pub fn start(&mut self) {
        if self.link.is_some() || !cfg!(target_os = "macos") {
            return;
        }
        let link = Arc::new(Link::default());
        let theirs = Arc::clone(&link);
        let spawned = std::thread::Builder::new()
            .name("quackapella-midi-out".into())
            .spawn(move || sender(&theirs));
        if spawned.is_ok() {
            self.link = Some(link);
        }
    }

    /// Audio thread: whether the source should exist.
    pub fn set_enabled(&self, on: bool) {
        if let Some(l) = &self.link {
            l.enabled.store(on, Ordering::Relaxed);
        }
    }

    /// Audio thread: queue one message.
    pub fn send(&self, m: Msg) {
        if let Some(l) = &self.link
            && l.enabled.load(Ordering::Relaxed)
        {
            l.queue.push(m);
        }
    }
}

impl Drop for VirtualPort {
    fn drop(&mut self) {
        if let Some(l) = &self.link {
            l.stop.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(target_os = "macos")]
fn sender(link: &Link) {
    use std::time::Duration;
    let mut port: Option<coremidi::Source> = None;
    while !link.stop.load(Ordering::Relaxed) {
        let on = link.enabled.load(Ordering::Relaxed);
        if on && port.is_none() {
            port = coremidi::Source::create(PORT_NAME);
        }
        if let Some(p) = &port {
            let mut batch = [0u8; 384];
            let mut n = 0;
            while let Some(m) = link.queue.pop() {
                batch[n..n + 3].copy_from_slice(&m.bytes());
                n += 3;
                if n + 3 > batch.len() {
                    p.send(&batch[..n]);
                    n = 0;
                }
            }
            if n > 0 {
                p.send(&batch[..n]);
            }
        } else {
            while link.queue.pop().is_some() {}
        }
        if !on && port.is_some() {
            port = None;
        }
        std::thread::sleep(Duration::from_millis(if on { 1 } else { 20 }));
    }
}

#[cfg(not(target_os = "macos"))]
fn sender(_: &Link) {}

/// Minimal CoreMIDI bindings: a client with one virtual source.
#[cfg(target_os = "macos")]
pub mod coremidi {
    use std::ffi::{CString, c_char, c_void};

    type CFStringRef = *const c_void;
    type OSStatus = i32;
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "CoreMIDI", kind = "framework")]
    unsafe extern "C" {
        static kMIDIPropertyName: CFStringRef;
        fn MIDIClientCreate(
            name: CFStringRef,
            notify: *const c_void,
            refcon: *mut c_void,
            out: *mut u32,
        ) -> OSStatus;
        fn MIDIClientDispose(client: u32) -> OSStatus;
        fn MIDISourceCreate(client: u32, name: CFStringRef, out: *mut u32) -> OSStatus;
        fn MIDIEndpointDispose(endpoint: u32) -> OSStatus;
        fn MIDIPacketListInit(list: *mut c_void) -> *mut c_void;
        fn MIDIPacketListAdd(
            list: *mut c_void,
            list_size: usize,
            cur: *mut c_void,
            time: u64,
            n: usize,
            data: *const u8,
        ) -> *mut c_void;
        fn MIDIReceived(src: u32, list: *const c_void) -> OSStatus;
        fn MIDIGetNumberOfSources() -> usize;
        fn MIDIGetSource(i: usize) -> u32;
        fn MIDIObjectGetStringProperty(
            obj: u32,
            prop: CFStringRef,
            out: *mut CFStringRef,
        ) -> OSStatus;
    }

    fn cfstr(s: &str) -> CFStringRef {
        let c = CString::new(s).unwrap_or_default();
        unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
    }

    /// A virtual MIDI source, disposed on drop.
    pub struct Source {
        client: u32,
        endpoint: u32,
    }

    impl Source {
        #[must_use]
        pub fn create(name: &str) -> Option<Self> {
            let n = cfstr(name);
            if n.is_null() {
                return None;
            }
            let mut client = 0;
            let mut endpoint = 0;
            let ok = unsafe {
                MIDIClientCreate(n, std::ptr::null(), std::ptr::null_mut(), &raw mut client) == 0
                    && MIDISourceCreate(client, n, &raw mut endpoint) == 0
            };
            unsafe { CFRelease(n) };
            if ok {
                Some(Self { client, endpoint })
            } else {
                if client != 0 {
                    unsafe { MIDIClientDispose(client) };
                }
                None
            }
        }

        /// Send complete channel messages, now.
        pub fn send(&self, bytes: &[u8]) {
            // A MIDIPacketList with one packet: 4 + 10 + data bytes.
            #[repr(C, align(8))]
            struct Buf([u8; 512]);
            let mut buf = Buf([0; 512]);
            let list = buf.0.as_mut_ptr().cast::<c_void>();
            unsafe {
                let cur = MIDIPacketListInit(list);
                let cur = MIDIPacketListAdd(list, 512, cur, 0, bytes.len(), bytes.as_ptr());
                if !cur.is_null() {
                    MIDIReceived(self.endpoint, list);
                }
            }
        }
    }

    impl Drop for Source {
        fn drop(&mut self) {
            unsafe {
                MIDIEndpointDispose(self.endpoint);
                MIDIClientDispose(self.client);
            }
        }
    }

    /// Names of every MIDI source the system sees.
    #[must_use]
    pub fn source_names() -> Vec<String> {
        let mut names = Vec::new();
        unsafe {
            for i in 0..MIDIGetNumberOfSources() {
                let mut s: CFStringRef = std::ptr::null();
                if MIDIObjectGetStringProperty(MIDIGetSource(i), kMIDIPropertyName, &raw mut s) == 0
                    && !s.is_null()
                {
                    let mut buf = [0 as c_char; 256];
                    if CFStringGetCString(s, buf.as_mut_ptr(), 256, UTF8) != 0 {
                        names.push(
                            std::ffi::CStr::from_ptr(buf.as_ptr())
                                .to_string_lossy()
                                .into_owned(),
                        );
                    }
                    CFRelease(s);
                }
            }
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_keeps_order_and_bounds() {
        let q = Queue::default();
        for i in 0..QUEUE {
            #[allow(clippy::cast_possible_truncation)]
            let n = (i % 128) as u8;
            assert!(q.push(Msg::note_on(0, n, 1)));
        }
        assert!(!q.push(Msg::note_on(0, 1, 1)));
        for i in 0..QUEUE {
            #[allow(clippy::cast_possible_truncation)]
            let want = (i % 128) as u8;
            assert_eq!(q.pop().map(|m| m.data1), Some(want));
        }
        assert_eq!(q.pop(), None);
    }

    /// The source shows up system-wide while MIDI Out is on.
    #[cfg(target_os = "macos")]
    #[test]
    fn virtual_source_appears_and_goes() {
        let name = "Quackapella Test Port";
        let src = coremidi::Source::create(name).expect("CoreMIDI source");
        assert!(coremidi::source_names().iter().any(|n| n == name));
        src.send(&Msg::note_on(0, 60, 100).bytes());
        drop(src);
        assert!(!coremidi::source_names().iter().any(|n| n == name));
    }
}
