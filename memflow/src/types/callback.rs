use core::ffi::c_void;

// C style callbacks that are needed so that C code can easily use callback like functions
#[repr(transparent)]
pub struct OpaqueCallback<'a, T>(Callback<'a, c_void, T>);

impl<T> OpaqueCallback<'_, T> {
    pub fn call(&mut self, arg: T) {
        (self.0.func)(self.0.data, arg);
    }
}

#[repr(C)]
pub struct Callback<'a, T, F> {
    data: &'a mut T,
    func: extern "C" fn(&mut T, F),
}

impl<'a, T, F> From<Callback<'a, T, F>> for OpaqueCallback<'a, F> {
    fn from(callback: Callback<'a, T, F>) -> Self {
        Self(callback.into_opaque())
    }
}

impl<'a, T, F> Callback<'a, T, F> {
    pub fn into_opaque(self) -> Callback<'a, c_void, F> {
        unsafe {
            Callback {
                data: std::mem::transmute(self.data),
                func: std::mem::transmute(self.func),
            }
        }
    }

    pub fn new(data: &'a mut T, func: extern "C" fn(&mut T, F)) -> Self {
        Self { data, func }
    }
}

impl<'a, T: FnOnce(F), F> From<&'a mut T> for OpaqueCallback<'a, F> {
    fn from(func: &'a mut T) -> Self {
        extern "C" fn callback<T: FnOnce(F), F>(func: &mut T, data: F) {
            func(data);
        }

        Callback {
            data: func,
            func: callback::<T, F>,
        }
        .into()
    }
}
