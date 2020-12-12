use darling::FromMeta;
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, AttributeArgs, Data, DeriveInput, Fields, ItemFn, ReturnType, Type};

#[derive(Debug, FromMeta)]
struct ConnectorFactoryArgs {
    name: String,
    #[darling(default)]
    version: Option<String>,
}

fn parse_resulting_type(output: &ReturnType) -> Option<syn::Type> {
    // There is a return type
    let ty = if let ReturnType::Type(_, ty) = output {
        ty
    } else {
        return None;
    };

    // Return type is a specific type
    let ty = if let Type::Path(ty) = &**ty {
        ty
    } else {
        return None;
    };

    // Take the first segment
    let first = &ty.path.segments.first()?;

    // It is a bracketed segment (for generic type)
    let args = if let syn::PathArguments::AngleBracketed(args) = &first.arguments {
        args
    } else {
        return None;
    };

    // There is an argument (Result<T, ...>)
    let first_arg = args.args.first()?;

    // It is a type
    if let syn::GenericArgument::Type(arg) = &first_arg {
        Some(arg.clone())
    } else {
        None
    }
}

// We should add conditional compilation for the crate-type here
// so our rust libraries who use a connector wont export those functions
// again by themselves (e.g. the ffi).
//
// This would also lead to possible duplicated symbols if
// multiple connectors are imported.
//
// See https://github.com/rust-lang/rust/issues/20267 for the tracking issue.
//
// #[cfg(crate_type = "cdylib")]
#[proc_macro_attribute]
pub fn connector(args: TokenStream, input: TokenStream) -> TokenStream {
    let attr_args = parse_macro_input!(args as AttributeArgs);
    let args = match ConnectorFactoryArgs::from_list(&attr_args) {
        Ok(v) => v,
        Err(e) => return TokenStream::from(e.write_errors()),
    };

    let connector_name = args.name;

    let func = parse_macro_input!(input as ItemFn);
    let func_name = &func.sig.ident;

    let connector_type = parse_resulting_type(&func.sig.output).expect("invalid return type");

    let create_gen = if func.sig.inputs.len() > 1 {
        quote! {
            #[doc(hidden)]
            extern "C" fn mf_create(
                args: *const ::std::os::raw::c_char,
                log_level: i32,
            ) -> std::option::Option<&'static mut ::std::ffi::c_void> {
                let level = match log_level {
                    0 => ::log::Level::Error,
                    1 => ::log::Level::Warn,
                    2 => ::log::Level::Info,
                    3 => ::log::Level::Debug,
                    4 => ::log::Level::Trace,
                    _ => ::log::Level::Trace,
                };

                let argsstr = unsafe { ::std::ffi::CStr::from_ptr(args) }.to_str()
                    .or_else(|e| {
                        ::log::error!("error converting connector args: {}", e);
                        Err(e)
                    })
                    .ok()?;
                let conn_args = ::memflow::connector::ConnectorArgs::parse(argsstr)
                    .or_else(|e| {
                        ::log::error!("error parsing connector args: {}", e);
                        Err(e)
                    })
                    .ok()?;

                let conn = Box::new(#func_name(&conn_args, level)
                    .or_else(|e| {
                        ::log::error!("{}", e);
                        Err(e)
                    })
                    .ok()?);
                Some(unsafe { &mut *(Box::into_raw(conn) as *mut ::std::ffi::c_void) })
            }
        }
    } else {
        quote! {
            #[doc(hidden)]
            extern "C" fn mf_create(
                args: *const ::std::os::raw::c_char,
                _: i32,
            ) -> std::option::Option<&'static mut ::std::ffi::c_void> {
                let argsstr = unsafe { ::std::ffi::CStr::from_ptr(args) }.to_str()
                    .or_else(|e| {
                        Err(e)
                    })
                    .ok()?;
                let conn_args = ::memflow::connector::ConnectorArgs::parse(argsstr)
                    .or_else(|e| {
                        Err(e)
                    })
                    .ok()?;

                let conn = Box::new(#func_name(&conn_args)
                    .or_else(|e| {
                        Err(e)
                    })
                    .ok()?);
                Some(unsafe { &mut *(Box::into_raw(conn) as *mut ::std::ffi::c_void) })
            }
        }
    };

    let mut gen = quote! {
        #[doc(hidden)]
        #[no_mangle]
        pub static MEMFLOW_CONNECTOR: ::memflow::connector::ConnectorDescriptor = ::memflow::connector::ConnectorDescriptor {
            connector_version: ::memflow::connector::MEMFLOW_CONNECTOR_VERSION,
            name: #connector_name,
            vtable: ::memflow::connector::ConnectorFunctionTable {
                create: mf_create,

                phys_read_raw_list: mf_phys_read_raw_list,
                phys_write_raw_list: mf_phys_write_raw_list,
                metadata: mf_metadata,

                clone: mf_clone,

                drop: mf_drop,
            },
        };

        #[doc(hidden)]
        extern "C" fn mf_phys_read_raw_list(
            phys_mem: &mut ::std::ffi::c_void,
            read_data: *mut ::memflow::mem::PhysicalReadData,
            read_data_count: usize,
        ) -> i32 {
            use ::memflow::mem::PhysicalMemory;

            let conn = unsafe { &mut *(phys_mem as *mut ::std::ffi::c_void as *mut #connector_type) };
            let read_data_slice = unsafe { std::slice::from_raw_parts_mut(read_data, read_data_count) };
            match conn.phys_read_raw_list(read_data_slice) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        }

        #[doc(hidden)]
        extern "C" fn mf_phys_write_raw_list(
            phys_mem: &mut ::std::ffi::c_void,
            write_data: *const ::memflow::mem::PhysicalWriteData,
            write_data_count: usize,
        ) -> i32 {
            use ::memflow::mem::PhysicalMemory;

            let conn = unsafe { &mut *(phys_mem as *mut ::std::ffi::c_void as *mut #connector_type) };
            let write_data_slice =
                unsafe { std::slice::from_raw_parts(write_data, write_data_count) };
            match conn.phys_write_raw_list(write_data_slice) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        }

        #[doc(hidden)]
        extern "C" fn mf_metadata(phys_mem: &::std::ffi::c_void) -> ::memflow::mem::PhysicalMemoryMetadata {
            use ::memflow::mem::PhysicalMemory;

            let conn = unsafe { &*(phys_mem as *const ::std::ffi::c_void as *const #connector_type) };
            let metadata = conn.metadata();
            metadata
        }

        #[doc(hidden)]
        extern "C" fn mf_clone(
            phys_mem: &::std::ffi::c_void,
        ) -> std::option::Option<&'static mut ::std::ffi::c_void> {
            let conn = unsafe { &*(phys_mem as *const ::std::ffi::c_void as *const #connector_type) };
            let cloned_conn = Box::new(conn.clone());
            Some(unsafe { &mut *(Box::into_raw(cloned_conn) as *mut ::std::ffi::c_void) })
        }

        #[doc(hidden)]
        extern "C" fn mf_drop(phys_mem: &mut ::std::ffi::c_void) {
            let _: Box<#connector_type> = unsafe { Box::from_raw(::std::mem::transmute(phys_mem)) };
            // drop box
        }

        #func
    };

    gen.extend(create_gen);

    gen.into()
}

#[proc_macro_derive(ByteSwap)]
pub fn byteswap_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let mut gen_inner = quote!();
    match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(named) => {
                for field in named.named.iter() {
                    let name = field.ident.as_ref().unwrap();
                    gen_inner.extend(quote!(
                        self.#name.byte_swap();
                    ));
                }
            }
            _ => unimplemented!(),
        },
        _ => unimplemented!(),
    };

    let gen = quote!(
        impl #impl_generics ::memflow::types::byte_swap::ByteSwap for #name #ty_generics #where_clause {
            fn byte_swap(&mut self) {
                #gen_inner
            }
        }
    );

    gen.into()
}
