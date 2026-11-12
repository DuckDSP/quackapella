//! The CLAP build seen from a minimal host, for what clap-validator
//! checks and the in-process adapter tests can't see: state round
//! trips through `clap.state`, display text, latency notifications.
#![cfg(feature = "clap")]
#![allow(unsafe_code, clippy::missing_safety_doc)]

use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, clap_event_header, clap_event_param_value,
    clap_input_events, clap_output_events,
};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_host_latency};
use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_param_info, clap_plugin_params};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

// Keep the plugin crate linked so its `clap_entry` symbol is present.
use quack_plugin as _;

unsafe extern "C" {
    static clap_entry: clap_plugin_entry;
}

/// What the host callbacks saw, per instance (`host_data`).
#[derive(Default)]
struct HostLog {
    latency_changed: AtomicU32,
    restart: AtomicU32,
    active: AtomicBool,
    changed_while_active: AtomicU32,
}

unsafe fn log(host: *const clap_host) -> &'static HostLog {
    unsafe { &*(*host).host_data.cast::<HostLog>() }
}

unsafe extern "C" fn latency_changed(host: *const clap_host) {
    let log = unsafe { log(host) };
    log.latency_changed.fetch_add(1, Ordering::SeqCst);
    if log.active.load(Ordering::SeqCst) {
        log.changed_while_active.fetch_add(1, Ordering::SeqCst);
    }
}
static HOST_LATENCY: clap_host_latency = clap_host_latency {
    changed: Some(latency_changed),
};
unsafe extern "C" fn get_extension(_: *const clap_host, id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_EXT_LATENCY {
        return (&raw const HOST_LATENCY).cast();
    }
    ptr::null()
}
unsafe extern "C" fn request_restart(host: *const clap_host) {
    unsafe { log(host) }.restart.fetch_add(1, Ordering::SeqCst);
}
unsafe extern "C" fn noop(_: *const clap_host) {}

/// The plugin, its host struct (kept alive) and what the host saw.
#[allow(dead_code)]
struct Plugin(*const clap_plugin, Box<clap_host>, &'static HostLog);

impl Plugin {
    fn new() -> Self {
        unsafe {
            let entry = &clap_entry;
            assert!((entry.init.unwrap())(c"".as_ptr()));
            let factory = (entry.get_factory.unwrap())(CLAP_PLUGIN_FACTORY_ID.as_ptr())
                .cast::<clap_plugin_factory>();
            let desc = ((*factory).get_plugin_descriptor.unwrap())(factory, 0);
            let log: &'static HostLog = Box::leak(Box::default());
            let host = Box::new(clap_host {
                clap_version: CLAP_VERSION,
                host_data: ptr::from_ref(log).cast_mut().cast(),
                name: c"test".as_ptr(),
                vendor: c"test".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: Some(get_extension),
                request_restart: Some(request_restart),
                request_process: Some(noop),
                request_callback: Some(noop),
            });
            let p = ((*factory).create_plugin.unwrap())(factory, &raw const *host, (*desc).id);
            assert!(!p.is_null());
            assert!(((*p).init.unwrap())(p));
            Self(p, host, log)
        }
    }

    fn ext<T>(&self, id: &CStr) -> &T {
        unsafe { &*((*self.0).get_extension.unwrap())(self.0, id.as_ptr()).cast::<T>() }
    }

    fn params(&self) -> Vec<clap_param_info> {
        let ext: &clap_plugin_params = self.ext(CLAP_EXT_PARAMS);
        let n = unsafe { (ext.count.unwrap())(self.0) };
        (0..n)
            .map(|i| unsafe {
                let mut info = std::mem::zeroed::<clap_param_info>();
                assert!((ext.get_info.unwrap())(self.0, i, &raw mut info));
                info
            })
            .collect()
    }

    fn get(&self, id: u32) -> f64 {
        let ext: &clap_plugin_params = self.ext(CLAP_EXT_PARAMS);
        let mut v = f64::NAN;
        assert!(unsafe { (ext.get_value.unwrap())(self.0, id, &raw mut v) });
        v
    }

    fn text(&self, id: u32, value: f64) -> String {
        let ext: &clap_plugin_params = self.ext(CLAP_EXT_PARAMS);
        let mut buf = [0 as c_char; 256];
        assert!(unsafe {
            (ext.value_to_text.unwrap())(self.0, id, value, buf.as_mut_ptr(), buf.len() as u32)
        });
        unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    /// Set values through `params.flush` (inactive plugin).
    fn flush(&self, values: &[(u32, f64)]) {
        let events: Vec<clap_event_param_value> =
            values.iter().map(|&(id, v)| param_event(id, v)).collect();
        let input = clap_input_events {
            ctx: (&raw const events).cast_mut().cast(),
            size: Some(in_size),
            get: Some(in_get),
        };
        let output = clap_output_events {
            ctx: ptr::null_mut(),
            try_push: Some(out_push),
        };
        let ext: &clap_plugin_params = self.ext(CLAP_EXT_PARAMS);
        unsafe { (ext.flush.unwrap())(self.0, &raw const input, &raw const output) };
    }

    fn save(&self) -> Vec<u8> {
        let ext: &clap_plugin_state = self.ext(CLAP_EXT_STATE);
        let mut out = Vec::<u8>::new();
        let stream = clap_ostream {
            ctx: (&raw mut out).cast(),
            write: Some(write),
        };
        assert!(unsafe { (ext.save.unwrap())(self.0, &raw const stream) });
        out
    }

    fn load(&self, blob: &[u8]) {
        let ext: &clap_plugin_state = self.ext(CLAP_EXT_STATE);
        let mut cursor = (blob, 0usize);
        let stream = clap_istream {
            ctx: (&raw mut cursor).cast(),
            read: Some(read),
        };
        assert!(unsafe { (ext.load.unwrap())(self.0, &raw const stream) });
    }

    fn activate(&self) {
        // The host side marks itself active first: `changed` from
        // inside `activate` is allowed, so it doesn't count.
        assert!(unsafe { ((*self.0).activate.unwrap())(self.0, 48_000.0, 1, 512) });
        self.2.active.store(true, Ordering::SeqCst);
    }

    fn deactivate(&self) {
        self.2.active.store(false, Ordering::SeqCst);
        unsafe { ((*self.0).deactivate.unwrap())(self.0) };
    }

    /// One 512-frame block of silence through `process`, with `values`
    /// as parameter events at frame 0.
    fn process(&self, values: &[(u32, f64)]) {
        const N: usize = 512;
        let mut chans = vec![vec![0.0f32; N]; 6];
        let mut ptrs: Vec<*mut f32> = chans.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let buf = |p: *mut *mut f32| clap_sys::audio_buffer::clap_audio_buffer {
            data32: p,
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let ins = [
            buf(ptrs.as_mut_ptr()),
            buf(unsafe { ptrs.as_mut_ptr().add(2) }),
        ];
        let mut outs = [buf(unsafe { ptrs.as_mut_ptr().add(4) })];
        let events: Vec<clap_event_param_value> =
            values.iter().map(|&(id, v)| param_event(id, v)).collect();
        let input = clap_input_events {
            ctx: (&raw const events).cast_mut().cast(),
            size: Some(in_size),
            get: Some(in_get),
        };
        let output = clap_output_events {
            ctx: ptr::null_mut(),
            try_push: Some(out_push),
        };
        let process = clap_sys::process::clap_process {
            steady_time: 0,
            frames_count: N as u32,
            transport: ptr::null(),
            audio_inputs: ins.as_ptr(),
            audio_outputs: outs.as_mut_ptr(),
            audio_inputs_count: 2,
            audio_outputs_count: 1,
            in_events: &raw const input,
            out_events: &raw const output,
        };
        unsafe {
            assert!(((*self.0).start_processing.unwrap())(self.0));
            ((*self.0).process.unwrap())(self.0, &raw const process);
            ((*self.0).stop_processing.unwrap())(self.0);
        }
    }

    fn latency(&self) -> u32 {
        let ext: &clap_sys::ext::latency::clap_plugin_latency = self.ext(CLAP_EXT_LATENCY);
        unsafe { (ext.get.unwrap())(self.0) }
    }

    fn on_main_thread(&self) {
        unsafe { ((*self.0).on_main_thread.unwrap())(self.0) };
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        unsafe { ((*self.0).destroy.unwrap())(self.0) };
    }
}

fn param_event(id: u32, value: f64) -> clap_event_param_value {
    clap_event_param_value {
        header: clap_event_header {
            size: size_of::<clap_event_param_value>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id: id,
        cookie: ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    }
}

unsafe extern "C" fn in_size(list: *const clap_input_events) -> u32 {
    unsafe { (*(*list).ctx.cast::<Vec<clap_event_param_value>>()).len() as u32 }
}
unsafe extern "C" fn in_get(list: *const clap_input_events, i: u32) -> *const clap_event_header {
    unsafe {
        let v = &*(*list).ctx.cast::<Vec<clap_event_param_value>>();
        (&raw const v[i as usize]).cast()
    }
}
unsafe extern "C" fn out_push(_: *const clap_output_events, _: *const clap_event_header) -> bool {
    true
}
unsafe extern "C" fn write(s: *const clap_ostream, buf: *const c_void, n: u64) -> i64 {
    unsafe {
        let out = &mut *(*s).ctx.cast::<Vec<u8>>();
        out.extend_from_slice(std::slice::from_raw_parts(buf.cast::<u8>(), n as usize));
    }
    n as i64
}
unsafe extern "C" fn read(s: *const clap_istream, buf: *mut c_void, n: u64) -> i64 {
    unsafe {
        let (blob, pos) = &mut *(*s).ctx.cast::<(&[u8], usize)>();
        let k = (blob.len() - *pos).min(n as usize);
        ptr::copy_nonoverlapping(blob[*pos..].as_ptr(), buf.cast::<u8>(), k);
        *pos += k;
        k as i64
    }
}

/// Every parameter moved off its default, as clap-validator does.
fn off_default(infos: &[clap_param_info]) -> Vec<(u32, f64)> {
    infos
        .iter()
        .map(|i| {
            let span = i.max_value - i.min_value;
            let mut v = i.min_value + span * 0.37;
            if i.flags & clap_sys::ext::params::CLAP_PARAM_IS_STEPPED != 0 {
                v = v.round();
                if (v - i.default_value).abs() < 0.5 {
                    v = if v + 1.0 <= i.max_value {
                        v + 1.0
                    } else {
                        v - 1.0
                    };
                }
            }
            (i.id, v)
        })
        .collect()
}

#[test]
fn state_round_trip_restores_every_parameter() {
    let a = Plugin::new();
    let infos = a.params();
    let wanted = off_default(&infos);
    a.flush(&wanted);
    let read_a: Vec<f64> = wanted.iter().map(|&(id, _)| a.get(id)).collect();
    let blob = a.save();

    let b = Plugin::new();
    b.load(&blob);
    for (&(id, _), &expect) in wanted.iter().zip(&read_a) {
        let got = b.get(id);
        assert!(
            (got - expect).abs() < 1e-6,
            "param {id}: {got} after load, saved {expect}"
        );
    }
    assert_eq!(b.save(), blob, "state not reproducible");
}

#[test]
fn percent_parameters_read_as_percent() {
    let p = Plugin::new();
    assert_eq!(p.text(2, 100.0), "100%");
    assert_eq!(p.text(2, 37.0), "37%");
}

/// clap-validator's path: values sent through `process`, then
/// save, reload into a fresh instance and read back.
#[test]
fn state_round_trip_after_process() {
    let a = Plugin::new();
    let wanted = off_default(&a.params());
    a.activate();
    a.process(&wanted);
    a.deactivate();
    let read_a: Vec<f64> = wanted.iter().map(|&(id, _)| a.get(id)).collect();
    for (&(id, v), &got) in wanted.iter().zip(&read_a) {
        assert!(
            (got - v).abs() < 1e-6,
            "param {id}: {got} after process, sent {v}"
        );
    }
    let blob = a.save();

    let b = Plugin::new();
    b.load(&blob);
    for (&(id, _), &expect) in wanted.iter().zip(&read_a) {
        let got = b.get(id);
        assert!(
            (got - expect).abs() < 1e-6,
            "param {id}: {got} after load, saved {expect}"
        );
    }
    b.activate();
    b.process(&[]);
    b.deactivate();
    for (&(id, _), &expect) in wanted.iter().zip(&read_a) {
        let got = b.get(id);
        assert!(
            (got - expect).abs() < 1e-6,
            "param {id}: {got} after load + process, saved {expect}"
        );
    }
    assert_eq!(b.save(), blob, "state not reproducible");
}

/// Low Latency (id 101) moved while active: CLAP wants a restart
/// request, and `latency.changed` only from inside `activate`.
#[test]
fn latency_change_while_active_goes_through_a_restart() {
    let p = Plugin::new();
    p.activate();
    let before = p.latency();
    p.process(&[(101, 1.0)]);
    p.on_main_thread();
    assert_eq!(
        p.2.restart.load(Ordering::SeqCst),
        1,
        "no restart requested"
    );
    assert_eq!(p.2.changed_while_active.load(Ordering::SeqCst), 0);
    let changed = p.2.latency_changed.load(Ordering::SeqCst);
    p.deactivate();
    p.activate();
    assert_ne!(p.latency(), before, "Low Latency should lower the latency");
    assert_eq!(p.2.latency_changed.load(Ordering::SeqCst), changed + 1);
    assert_eq!(p.2.changed_while_active.load(Ordering::SeqCst), 0);
    p.deactivate();
}
