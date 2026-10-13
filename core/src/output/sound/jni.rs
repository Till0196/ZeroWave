// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Just enough JNI to call eight methods (the NDK has no compressed-format track).
//! Table entries are named by index as in `jni.h`; [`Env::sane`] checks before any call.

use std::ffi::{CString, c_char, c_int, c_void};

use crate::output::sound::Error;

const RESERVED: usize = 4;
/// Functions after the reserved slots, as of JNI 1.6.
const FUNCTIONS: usize = 229;

mod at {
    pub const GET_VERSION: usize = 0;
    pub const FIND_CLASS: usize = 2;
    pub const EXCEPTION_OCCURRED: usize = 11;
    pub const EXCEPTION_DESCRIBE: usize = 12;
    pub const EXCEPTION_CLEAR: usize = 13;
    pub const NEW_GLOBAL_REF: usize = 17;
    pub const DELETE_GLOBAL_REF: usize = 18;
    pub const DELETE_LOCAL_REF: usize = 19;
    pub const NEW_OBJECT_A: usize = 26;
    pub const GET_METHOD_ID: usize = 29;
    pub const CALL_INT_METHOD_A: usize = 47;
    pub const CALL_VOID_METHOD_A: usize = 59;
    pub const GET_STATIC_METHOD_ID: usize = 109;
    pub const CALL_STATIC_INT_METHOD_A: usize = 127;
    pub const NEW_SHORT_ARRAY: usize = 174;
    pub const SET_SHORT_ARRAY_REGION: usize = 206;
}

pub type Object = *mut c_void;
pub type Method = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub union Value {
    pub int: i32,
    pub object: Object,
    /// Never read; makes the union eight bytes wide on every machine.
    _wide: i64,
}

impl Value {
    pub fn int(value: i32) -> Self {
        Self { int: value }
    }

    pub fn object(value: Object) -> Self {
        Self { object: value }
    }
}

#[repr(C)]
struct Interface {
    table: [*const c_void; RESERVED + FUNCTIONS],
}

#[repr(C)]
struct Invoke {
    _reserved: [*const c_void; 3],
    destroy: *const c_void,
    attach: unsafe extern "C" fn(*mut c_void, *mut *mut c_void, *mut c_void) -> c_int,
    detach: unsafe extern "C" fn(*mut c_void) -> c_int,
    get_env: unsafe extern "C" fn(*mut c_void, *mut *mut c_void, c_int) -> c_int,
    _attach_daemon: *const c_void,
}

const VERSION_1_6: c_int = 0x0001_0006;
const OK: c_int = 0;

/// Detaches on drop if attached here.
#[derive(Debug)]
pub struct Env {
    vm: *mut c_void,
    env: *mut c_void,
    ours: bool,
}

impl Env {
    /// # Safety
    ///
    /// `vm` has to be a `JavaVM *` the machine gave out.
    pub unsafe fn attach(vm: *mut c_void) -> Result<Self, Error> {
        if vm.is_null() {
            return Err(Error::Unavailable(String::from("JavaVM が無い")));
        }
        // SAFETY: `vm` points to the invoke table, by the contract above.
        let invoke = unsafe { &**vm.cast::<*const Invoke>() };
        let mut env: *mut c_void = std::ptr::null_mut();
        // SAFETY: `env` is a valid place and the version is one the machine knows.
        let status = unsafe { (invoke.get_env)(vm, &mut env, VERSION_1_6) };
        if status == OK && !env.is_null() {
            return Ok(Self {
                vm,
                env,
                ours: false,
            });
        }
        // SAFETY: null arguments ask for the default thread group and no name.
        let status = unsafe { (invoke.attach)(vm, &mut env, std::ptr::null_mut()) };
        if status != OK || env.is_null() {
            return Err(Error::Unavailable(format!(
                "このスレッドを JavaVM に繋げない ({status})"
            )));
        }
        Ok(Self {
            vm,
            env,
            ours: true,
        })
    }

    /// # Safety
    ///
    /// `T` has to be the signature `jni.h` declares for the entry at `index`.
    unsafe fn call<T: Copy>(&self, index: usize) -> T {
        debug_assert_eq!(size_of::<T>(), size_of::<*const c_void>());
        debug_assert!(index < FUNCTIONS);
        // SAFETY: `env` points at a pointer to the table, which outlives this attachment.
        let interface = unsafe { &**self.env.cast::<*const Interface>() };
        let pointer = interface.table[RESERVED + index];
        // SAFETY: a table entry is a function pointer of the same size.
        unsafe { *(&raw const pointer).cast::<T>() }
    }

    /// A wrong index is a call into the wrong function, which shows up as a vanished process.
    pub fn sane(&self) -> Result<(), Error> {
        // SAFETY: index nought is `GetVersion`, taking the environment and giving a `jint`.
        let version: unsafe extern "C" fn(*mut c_void) -> c_int =
            unsafe { self.call(at::GET_VERSION) };
        // SAFETY: the environment is this thread's.
        let version = unsafe { version(self.env) };
        if !(VERSION_1_6 - 2..=0x0001_00ff).contains(&version) {
            return Err(Error::Unavailable(format!(
                "JNIEnv から想定外のバージョンが返った ({version:#x})"
            )));
        }
        Ok(())
    }

    pub fn class(&self, name: &str) -> Result<Object, Error> {
        let text = CString::new(name).map_err(|_| Error::Unsupported("a class with a nought"))?;
        // SAFETY: the index is `FindClass` and the signature is its own.
        let find: unsafe extern "C" fn(*mut c_void, *const c_char) -> Object =
            unsafe { self.call(at::FIND_CLASS) };
        // SAFETY: the name outlives the call.
        let local = unsafe { find(self.env, text.as_ptr()) };
        self.checked()?;
        if local.is_null() {
            return Err(Error::Unavailable(format!("{name} が無い")));
        }
        let global = self.keep(local);
        self.drop_local(local);
        Ok(global)
    }

    pub fn method(&self, class: Object, name: &str, signature: &str) -> Result<Method, Error> {
        self.any_method(class, name, signature, at::GET_METHOD_ID)
    }

    pub fn static_method(
        &self,
        class: Object,
        name: &str,
        signature: &str,
    ) -> Result<Method, Error> {
        self.any_method(class, name, signature, at::GET_STATIC_METHOD_ID)
    }

    fn any_method(
        &self,
        class: Object,
        name: &str,
        signature: &str,
        index: usize,
    ) -> Result<Method, Error> {
        let named = CString::new(name).map_err(|_| Error::Unsupported("a method with a nought"))?;
        let signed =
            CString::new(signature).map_err(|_| Error::Unsupported("a type with a nought"))?;
        // SAFETY: both indices name a `Get*MethodID`, which have the same signature.
        let get: unsafe extern "C" fn(*mut c_void, Object, *const c_char, *const c_char) -> Method =
            unsafe { self.call(index) };
        // SAFETY: the class is live and both strings outlive the call.
        let method = unsafe { get(self.env, class, named.as_ptr(), signed.as_ptr()) };
        self.checked()?;
        if method.is_null() {
            return Err(Error::Unavailable(format!("{name}{signature} が無い")));
        }
        Ok(method)
    }

    pub fn new_object(
        &self,
        class: Object,
        method: Method,
        args: &[Value],
    ) -> Result<Object, Error> {
        // SAFETY: the index is `NewObjectA`.
        let new: unsafe extern "C" fn(*mut c_void, Object, Method, *const Value) -> Object =
            unsafe { self.call(at::NEW_OBJECT_A) };
        // SAFETY: the class and method go together and `args` matches the signature.
        let local = unsafe { new(self.env, class, method, args.as_ptr()) };
        self.checked()?;
        if local.is_null() {
            return Err(Error::Unavailable(String::from("new が null を返した")));
        }
        let global = self.keep(local);
        self.drop_local(local);
        Ok(global)
    }

    pub fn call_int(&self, object: Object, method: Method, args: &[Value]) -> Result<i32, Error> {
        // SAFETY: the index is `CallIntMethodA`.
        let call: unsafe extern "C" fn(*mut c_void, Object, Method, *const Value) -> i32 =
            unsafe { self.call(at::CALL_INT_METHOD_A) };
        // SAFETY: as in `new_object`.
        let answer = unsafe { call(self.env, object, method, args.as_ptr()) };
        self.checked()?;
        Ok(answer)
    }

    pub fn call_static_int(
        &self,
        class: Object,
        method: Method,
        args: &[Value],
    ) -> Result<i32, Error> {
        // SAFETY: the index is `CallStaticIntMethodA`.
        let call: unsafe extern "C" fn(*mut c_void, Object, Method, *const Value) -> i32 =
            unsafe { self.call(at::CALL_STATIC_INT_METHOD_A) };
        // SAFETY: as above.
        let answer = unsafe { call(self.env, class, method, args.as_ptr()) };
        self.checked()?;
        Ok(answer)
    }

    pub fn call_void(&self, object: Object, method: Method, args: &[Value]) -> Result<(), Error> {
        // SAFETY: the index is `CallVoidMethodA`.
        let call: unsafe extern "C" fn(*mut c_void, Object, Method, *const Value) =
            unsafe { self.call(at::CALL_VOID_METHOD_A) };
        // SAFETY: as above.
        unsafe { call(self.env, object, method, args.as_ptr()) };
        self.checked()
    }

    pub fn short_array(&self, length: usize) -> Result<Object, Error> {
        // SAFETY: the index is `NewShortArray`.
        let new: unsafe extern "C" fn(*mut c_void, i32) -> Object =
            unsafe { self.call(at::NEW_SHORT_ARRAY) };
        // SAFETY: the environment is this thread's.
        let local = unsafe { new(self.env, length as i32) };
        self.checked()?;
        if local.is_null() {
            return Err(Error::Unavailable(String::from("short[] が作れない")));
        }
        let global = self.keep(local);
        self.drop_local(local);
        Ok(global)
    }

    pub fn fill(&self, array: Object, values: &[i16]) -> Result<(), Error> {
        // SAFETY: the index is `SetShortArrayRegion`.
        let set: unsafe extern "C" fn(*mut c_void, Object, i32, i32, *const i16) =
            unsafe { self.call(at::SET_SHORT_ARRAY_REGION) };
        // SAFETY: the one caller never passes more than the array holds.
        unsafe { set(self.env, array, 0, values.len() as i32, values.as_ptr()) };
        self.checked()
    }

    fn keep(&self, local: Object) -> Object {
        // SAFETY: the index is `NewGlobalRef`.
        let new: unsafe extern "C" fn(*mut c_void, Object) -> Object =
            unsafe { self.call(at::NEW_GLOBAL_REF) };
        // SAFETY: `local` is live.
        unsafe { new(self.env, local) }
    }

    pub fn release(&self, global: Object) {
        if global.is_null() {
            return;
        }
        // SAFETY: the index is `DeleteGlobalRef`.
        let delete: unsafe extern "C" fn(*mut c_void, Object) =
            unsafe { self.call(at::DELETE_GLOBAL_REF) };
        // SAFETY: the reference was taken by `keep` and is given up once.
        unsafe { delete(self.env, global) };
    }

    fn drop_local(&self, local: Object) {
        // SAFETY: the index is `DeleteLocalRef`.
        let delete: unsafe extern "C" fn(*mut c_void, Object) =
            unsafe { self.call(at::DELETE_LOCAL_REF) };
        // SAFETY: `local` came from a call that made one.
        unsafe { delete(self.env, local) };
    }

    /// Prints the exception first: the `AudioTrack` constructor's message is the only place a
    /// device says what it refused.
    fn checked(&self) -> Result<(), Error> {
        // SAFETY: the indices are the three exception calls.
        let occurred: unsafe extern "C" fn(*mut c_void) -> Object =
            unsafe { self.call(at::EXCEPTION_OCCURRED) };
        // SAFETY: the environment is this thread's.
        let thrown = unsafe { occurred(self.env) };
        if thrown.is_null() {
            return Ok(());
        }
        // SAFETY: as above.
        let describe: unsafe extern "C" fn(*mut c_void) =
            unsafe { self.call(at::EXCEPTION_DESCRIBE) };
        let clear: unsafe extern "C" fn(*mut c_void) = unsafe { self.call(at::EXCEPTION_CLEAR) };
        // SAFETY: an exception is pending, which is what both want.
        unsafe {
            describe(self.env);
            clear(self.env);
        }
        self.drop_local(thrown);
        Err(Error::Unavailable(String::from(
            "Java 側が例外を投げた（logcat に出ている）",
        )))
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if !self.ours {
            return;
        }
        // SAFETY: `vm` is the machine attached to, detached once from the attaching thread.
        let invoke = unsafe { &**self.vm.cast::<*const Invoke>() };
        // SAFETY: as above.
        unsafe { (invoke.detach)(self.vm) };
    }
}
