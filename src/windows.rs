use crate::{Interests, Token, TOKEN};
use std::io::{self, Read, Write};
use std::net;
use std::os::windows::io::{AsRawSocket, RawSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::mem;

use super::LOOP_STOP_SIGNAL;

pub type Event = ffi::OVERLAPPED_ENTRY;
pub type Source = std::os::windows::io::RawSocket;


#[derive(Debug)]
pub struct TcpStream {
    inner: net::TcpStream,
    buffer: Vec<u8>,
    wsabuf: Vec<ffi::WSABUF>,
    event: Option<ffi::WSAOVERLAPPED>,
    token: Option<usize>,
    pos: usize,
    // status: TcpReadiness,
}

// enum TcpReadiness {
//     Ready,
//     NotReady,
// }

impl TcpStream {
    pub fn connect(adr: impl net::ToSocketAddrs) -> io::Result<Self> {
        // This is a shortcut since this will block when establishing the connection.
        // There are several ways of avoiding this.
        // a) Obtrain the socket using system calls, set it to non_blocking before we connect
        // b) use the crate [net2](https://docs.rs/net2/0.2.33/net2/index.html) which
        // defines a trait with default implementation for TcpStream which allow us to set
        // it to non-blocking before we connect

        // Rust creates a WSASocket set to overlapped by default which is just what we need
        // https://github.com/rust-lang/rust/blob/f86521e0a33a2b54c4c23dbfc5250013f7a33b11/src/libstd/sys/windows/net.rs#L99
        let stream = net::TcpStream::connect(adr)?;
        stream.set_nonblocking(true)?;

        let mut buffer = vec![0_u8; 1024];
        let wsabuf =  vec![ffi::WSABUF::new(buffer.len() as u32, buffer.as_mut_ptr())];
        Ok(TcpStream {
            inner: stream,
            buffer,
            wsabuf,
            event: None,
            token: None,
            pos: 0,
            //status: TcpReadiness::NotReady,
        })
    }

    pub fn source(&self) -> Source {
        self.inner.as_raw_socket()
    }
}

impl Read for TcpStream {
    fn read(&mut self, buff: &mut [u8]) -> io::Result<usize> {
    //   self.inner.read(buff)  
        let mut bytes_read = 0;
        if self.buffer.len() - bytes_read <= buff.len() {
            for (a, b) in self.buffer.iter().skip(self.pos).zip(buff) {
                *b = *a;
                bytes_read += 1;
            }

            Ok(bytes_read)
        } else {
            for (b, a) in buff.iter_mut().zip(&self.buffer) {
                *b = *a;
                bytes_read += 1;
            }
            self.pos += bytes_read;
            Ok(bytes_read)
        }
    }
}

impl Write for TcpStream {
    fn write(&mut self, buff: &[u8]) -> io::Result<usize> {
        self.inner.write(buff)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl AsRawSocket for TcpStream {
    fn as_raw_socket(&self) -> RawSocket {
        self.inner.as_raw_socket()
    }
}

pub struct Registrator {
    completion_port: isize,
}

impl Registrator {
        pub fn register(&self, soc: &mut TcpStream, token: usize, interests: Interests) -> io::Result<()> {
        println!("REGISTERING SOCKET WITH COMPLETION PORT");

        ffi::connect_socket_to_completion_port(soc.as_raw_socket(), self.completion_port, token)?;
        println!("REGISTERING BUFFER: {:?}", &soc.buffer[0..10]);
        //let mut evts = vec![0u8; 256];

        let event = ffi::create_soc_read_event(soc.as_raw_socket(), &mut soc.wsabuf)?;
        //soc.event = Some(event);
        //soc.token = Some(token);
        

        println!("EVENT REGISTERED: {:?}", soc.event);
        // ffi::register_event(self.completion_port, 256, token as u32, &mut read_event)?;
        //println!("READ_EVET_AFTER_REGISTER: {:?}", &read_event);
        Ok(())
    }

    pub fn close_loop(&self) -> io::Result<()> {
        let mut overlapped = ffi::WSAOVERLAPPED::zeroed();
        ffi::post_queued_completion_status(self.completion_port, 0, LOOP_STOP_SIGNAL, &mut overlapped)?;
        Ok(())

    }
}

// possible Arc<InnerSelector> needed
pub struct Selector {
    id: usize,
    completion_port: isize,
    buffers: Mutex<Vec<Vec<u8>>>,
}

impl Selector {
    pub fn new() -> io::Result<Self> {
        // set up the queue
        let completion_port = ffi::create_completion_port()?;
        let id = TOKEN.next();

        Ok(Selector {
            completion_port,
            id,
            buffers: Mutex::new(Vec::with_capacity(256)),
        })
    }

    pub fn registrator(&self) -> Registrator {
        Registrator {
            completion_port: self.completion_port,
        }
    }

    pub fn register(&self, soc: &mut TcpStream, token: usize, interests: Interests) -> io::Result<()> {
        println!("REGISTERING SOCKET WITH COMPLETION PORT");

        ffi::connect_socket_to_completion_port(soc.as_raw_socket(), self.completion_port, token)?;
        println!("REGISTERING BUFFER: {:?}", &soc.buffer[0..10]);
        //let mut evts = vec![0u8; 256];

        let event = ffi::create_soc_read_event(soc.as_raw_socket(), &mut soc.wsabuf)?;
        //soc.event = Some(event);
        //soc.token = Some(token);
        

        println!("EVENT REGISTERED: {:?}", soc.event);
        // ffi::register_event(self.completion_port, 256, token as u32, &mut read_event)?;
        //println!("READ_EVET_AFTER_REGISTER: {:?}", &read_event);
        Ok(())
    }

    /// Blocks until an Event has occured. Never times out. We could take a parameter
    /// for a timeout and pass it on but we'll not do that in our example.
    pub fn select(
        &mut self,
        events: &mut Vec<ffi::OVERLAPPED_ENTRY>,
        //awakener: Sender<usize>,
    ) -> io::Result<()> {
        // calling GetQueueCompletionStatus will either return a handle to a "port" ready to read or
        // block if the queue is empty.

        // first let's clear events for any previous events and wait until we get som more
        events.clear();
        let mut bytes = 0;
        let mut token: &mut usize = &mut 0;
        let ul_count = events.capacity() as u32;

        let removed = ffi::get_queued_completion_status_ex(
            self.completion_port as isize,
            events,
            ul_count,
            None,
            false,
        )?;

        unsafe {
            events.set_len(removed as usize);
        }

        Ok(())
    }
}

mod ffi {
    use super::*;
    use std::io;
    use std::os::raw::c_void;
    use std::os::windows::io::RawSocket;
    use std::ptr;
    use std::mem;

    #[derive(Debug, Clone)]
    pub struct IOCPevent {}

    impl Default for IOCPevent {
        fn default() -> Self {
            IOCPevent {}
        }
    }

    #[repr(C)]
    #[derive(Clone, Debug)]
    pub struct WSABUF {
        len: u32,
        buf: *mut u8,
    }

    impl WSABUF {
        fn as_vec(self) -> Vec<u8> {
            unsafe { Vec::from_raw_parts(self.buf, self.len as usize, self.len as usize) }
        }

        pub fn new(len: u32, buf: *mut u8) -> Self {
            WSABUF { len, buf }
        }
    }

    #[repr(C)]
    #[derive(Debug, Clone)]
    pub struct OVERLAPPED_ENTRY {
        // Normally a pointer but since it's just passed through we can store whatever valid usize we want. For our case
        // an Id or Token is more secure than dereferencing som part of memory later.
        lp_completion_key: *mut usize,
        pub lp_overlapped: *mut OVERLAPPED,
        internal: usize,
        bytes_transferred: u32,
    }

    impl OVERLAPPED_ENTRY {
        pub fn token(&self) -> Option<Token> {
            
            if self.lp_completion_key.is_null() {
                None
            } else {
                // since we only use this as a storage for integers in our implementation we just cast this
                // as an usize since it will NOT be a valid pointer.
                Some(Token::new(self.lp_completion_key as usize))
            }
        }

        pub fn zeroed() -> Self {
            OVERLAPPED_ENTRY {
            lp_completion_key: ptr::null_mut(),
            lp_overlapped: ptr::null_mut(),
            internal: 0,
            bytes_transferred: 0,
            }
        }
    }

    // Reference: https://docs.microsoft.com/en-us/windows/win32/api/winsock2/ns-winsock2-wsaoverlapped
    #[repr(C)]
    #[derive(Debug)]
    pub struct WSAOVERLAPPED {
        /// Reserved for internal use
        internal: ULONG_PTR,
        /// Reserved
        internal_high: ULONG_PTR,
        /// Reserved for service providers
        offset: DWORD,
        /// Reserved for service providers
        offset_high: DWORD,
        /// If an overlapped I/O operation is issued without an I/O completion routine
        /// (the operation's lpCompletionRoutine parameter is set to null), then this parameter
        /// should either contain a valid handle to a WSAEVENT object or be null. If the
        /// lpCompletionRoutine parameter of the call is non-null then applications are free
        /// to use this parameter as necessary.
        h_event: HANDLE,
    }

    impl WSAOVERLAPPED {
        pub fn zeroed() -> Self {
            WSAOVERLAPPED {
                internal: ptr::null_mut(),
                internal_high: ptr::null_mut(),
                offset: 0,
                offset_high: 0,
                h_event: 0,
            }
        }
    }

    // https://docs.microsoft.com/en-us/windows/win32/api/minwinbase/ns-minwinbase-overlapped
    #[repr(C)]
    #[derive(Debug)]
    pub struct OVERLAPPED {
        pub internal: ULONG_PTR,
        internal_high: ULONG_PTR,
        dummy: [DWORD; 2],
        h_event: HANDLE,
    }

    #[repr(C)]
    pub struct WSADATA {
        w_version: u16,
        i_max_sockets: u16,
        i_max_u_dp_dg: u16,
        lp_vendor_info: u8,
        sz_description: [u8; WSADESCRIPTION_LEN + 1],
        sz_system_status: [u8; WSASYS_STATUS_LEN + 1],
    }

    impl Default for WSADATA {
        fn default() -> WSADATA {
            WSADATA {
                w_version: 0,
                i_max_sockets: 0,
                i_max_u_dp_dg: 0,
                lp_vendor_info: 0,
                sz_description: [0_u8; WSADESCRIPTION_LEN + 1],
                sz_system_status: [0_u8; WSASYS_STATUS_LEN + 1],
            }
        }
    }

    /// See this for a thorough explanation: https://play.rust-lang.org/?version=stable&mode=debug&edition=2018&gist=b92fdec5f2fca0d05d47bb5ecbfb5f68
    fn makeword(low: u8, high: u8) -> u16 {
        ((high as u16) << 8) + low as u16
}

    // You can find most of these here: https://docs.microsoft.com/en-us/windows/win32/winprog/windows-data-types
    /// The HANDLE type is actually a `*mut c_void` but windows preserves backwards compatibility by allowing
    /// a INVALID_HANDLE_VALUE which is `-1`. We can't express that in Rust so it's much easier for us to treat
    /// this as an isize instead;
    pub type HANDLE = isize;
    pub type BOOL = bool;
    pub type WORD = u16;
    pub type DWORD = u32;
    pub type ULONG = u32;
    pub type PULONG = *mut ULONG;
    pub type ULONG_PTR = *mut usize;
    pub type PULONG_PTR = *mut ULONG_PTR;
    pub type LPDWORD = *mut DWORD;
    pub type LPWSABUF = *mut WSABUF;
    pub type LPWSAOVERLAPPED = *mut WSAOVERLAPPED;
    pub type LPOVERLAPPED = *mut OVERLAPPED;
    pub type LPWSAOVERLAPPED_COMPLETION_ROUTINE = *const fn();

    // https://referencesource.microsoft.com/#System.Runtime.Remoting/channels/ipc/win32namedpipes.cs,edc09ced20442fea,references
    // read this! https://devblogs.microsoft.com/oldnewthing/20040302-00/?p=40443
    /// Defined in `win32.h` which you can find on your windows system
    pub const INVALID_HANDLE_VALUE: HANDLE = -1;

    // https://docs.microsoft.com/en-us/windows/win32/winsock/windows-sockets-error-codes-2
    pub const WSA_IO_PENDING: i32 = 997;

    // This can also be written as `4294967295` if you look at sources on the internet.
    // Interpreted as an i32 the value is -1
    // see for yourself: https://play.rust-lang.org/?version=stable&mode=debug&edition=2018&gist=4b93de7d7eb43fa9cd7f5b60933d8935
    pub const INFINITE: u32 = 0xFFFFFFFF;

    pub const WSADESCRIPTION_LEN: usize = 256;
    pub const WSASYS_STATUS_LEN: usize = 128;

    #[link(name = "Kernel32")]
    extern "stdcall" {

        // https://docs.microsoft.com/en-us/windows/win32/fileio/createiocompletionport
        fn CreateIoCompletionPort(
            filehandle: HANDLE,
            existing_completionport: HANDLE,
            completion_key: ULONG_PTR,
            number_of_concurrent_threads: DWORD,
        ) -> HANDLE;
        // https://docs.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsarecv
        fn WSARecv(
            s: RawSocket,
            lpBuffers: LPWSABUF,
            dwBufferCount: DWORD,
            lpNumberOfBytesRecvd: LPDWORD,
            lpFlags: LPDWORD,
            lpOverlapped: LPWSAOVERLAPPED,
            lpCompletionRoutine: LPWSAOVERLAPPED_COMPLETION_ROUTINE,
        ) -> i32;
        // https://docs.microsoft.com/en-us/windows/win32/fileio/postqueuedcompletionstatus
        fn PostQueuedCompletionStatus(
            CompletionPort: HANDLE,
            dwNumberOfBytesTransferred: DWORD,
            dwCompletionKey: ULONG_PTR,
            lpOverlapped: LPWSAOVERLAPPED,
        ) -> i32;
        // https://docs.microsoft.com/nb-no/windows/win32/api/ioapiset/nf-ioapiset-getqueuedcompletionstatus
        fn GetQueuedCompletionStatusEx(
            CompletionPort: HANDLE,
            lpCompletionPortEntries: *mut OVERLAPPED_ENTRY,
            ulCount: ULONG,
            ulNumEntriesRemoved: PULONG,
            dwMilliseconds: DWORD,
            fAlertable: BOOL,
        ) -> i32;


        fn GetQueuedCompletionStatus(CompletionPort: HANDLE, lpNumberOfBytesTransferred: LPDWORD, lpCompletionKey: PULONG_PTR, lpOverlapped: LPOVERLAPPED, dwMilliseconds: DWORD) -> i32;

        // https://docs.microsoft.com/nb-no/windows/win32/api/handleapi/nf-handleapi-closehandle
        fn CloseHandle(hObject: HANDLE) -> i32;

        // https://docs.microsoft.com/nb-no/windows/win32/api/winsock/nf-winsock-wsagetlasterror
        fn WSAGetLastError() -> i32;
    }

    // ===== SAFE WRAPPERS =====

    pub fn close_handle(handle: isize) -> io::Result<()> {
        let res = unsafe { CloseHandle(handle) };

         if res == 0 {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(())
        }
    }

    pub fn create_completion_port() -> io::Result<isize> {
        unsafe {
            // number_of_concurrent_threads = 0 means use the number of physical threads but the argument is
            // ignored when existing_completionport is set to null.
            let res = CreateIoCompletionPort(INVALID_HANDLE_VALUE, 0, ptr::null_mut(), 0);
            if (res as *mut usize).is_null() {
                return Err(std::io::Error::last_os_error());
            }

            Ok(res)
        }
    }

    /// Returns the file handle to the completion port we passed in
    pub fn connect_socket_to_completion_port(s: RawSocket, completion_port: isize, token: usize) -> io::Result<isize> {
        let res = unsafe { CreateIoCompletionPort(s as isize, completion_port, token as *mut usize, 0)};

        if (res as *mut usize).is_null() {
                return Err(std::io::Error::last_os_error());
            }

        Ok(res)
    }

    /// Creates a socket read event.
    /// ## Returns
    /// The number of bytes recieved
    pub fn create_soc_read_event(
        s: RawSocket,
        wsabuffers: &mut [WSABUF],
    ) -> Result<WSAOVERLAPPED, io::Error> {

        //let mut ol: mem::MaybeUninit<WSAOVERLAPPED> = mem::MaybeUninit::zeroed();
        let mut ol = WSAOVERLAPPED::zeroed();        
        // This actually takes an array of buffers but we will only need one so we can just box it
        // and point to it (there is no difference in memory between a `vec![T; 1]` and a `Box::new(T)`)
        // let buff_ptr: *mut WSABUF = wsabuffers.as_mut_ptr();
        // let mut buffer = vec![0_u8; 256];
        // let mut b = WSABUF::new(256, buffer.as_mut_ptr()); 
        let mut bytes_recieved = 0;
        let mut flags = 0;
        
        //let num_bytes_recived_ptr: *mut u32 = bytes_recieved;
        let res = unsafe { WSARecv(s, wsabuffers.as_mut_ptr(), 1, ptr::null_mut(), &mut flags, &mut ol, ptr::null_mut()) };
        println!("WSARECV_OVERLAPPED: {:?}", ol);
        if res != 0 {
            let err = unsafe { WSAGetLastError() };
            if err == WSA_IO_PENDING {
                // Everything is OK, and we can wait this with GetQueuedCompletionStatus
                Ok(ol)
            } else {
                Err(std::io::Error::last_os_error())
            }
        } else {
            // The socket is already ready so we don't need to queue it
            // TODO: Avoid queueing this
            Ok(ol)
        }
    }

    pub fn post_queued_completion_status(
        completion_port: isize,
        bytes_to_transfer: u32,
        completion_key: usize,
        overlapped_ptr: &mut WSAOVERLAPPED,
    ) -> io::Result<()> {
        let res = unsafe {
            PostQueuedCompletionStatus(
                completion_port,
                bytes_to_transfer,
                completion_key as *mut usize,
                overlapped_ptr,
            )
        };
        if res == 0 {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(())
        }
    }

    /// ## Parameters:
    /// - *completion_port:* the handle to a completion port created by calling CreateIoCompletionPort
    /// - *completion_port_entries:* a pointer to an array of OVERLAPPED_ENTRY structures
    /// - *ul_count:* The maximum number of entries to remove
    /// - *timeout:* The timeout in milliseconds, if set to NONE, timeout is set to INFINITE
    /// - *alertable:* If this parameter is FALSE, the function does not return until the time-out period has elapsed or
    /// an entry is retrieved. If the parameter is TRUE and there are no available entries, the function performs
    /// an alertable wait. The thread returns when the system queues an I/O completion routine or APC to the thread
    /// and the thread executes the function.
    ///
    /// ## Returns
    /// The number of items actually removed from the queue
    pub fn get_queued_completion_status_ex(
        completion_port: isize,
        completion_port_entries: &mut [OVERLAPPED_ENTRY],
        ul_count: u32,
        timeout: Option<u32>,
        alertable: bool,
    ) -> io::Result<u32> {
        let mut ul_num_entries_removed: u32 = 0;
        // can't coerce directly to *mut *mut usize and cant cast `&mut` as `*mut`
        // let completion_key_ptr: *mut &mut usize = completion_key_ptr;
        // // but we can cast a `*mut ...`
        // let completion_key_ptr: *mut *mut usize = completion_key_ptr as *mut *mut usize;
        let timeout = timeout.unwrap_or(INFINITE);
        let res = unsafe {
            GetQueuedCompletionStatusEx(
                completion_port,
                completion_port_entries.as_mut_ptr(),
                ul_count,
                &mut ul_num_entries_removed,
                timeout,
                alertable,
            )
        };

        if res == 0 {   
            
            
        Err(std::io::Error::last_os_error())
        } else {
            Ok(ul_num_entries_removed)
        }
    }

    pub fn get_queued_completion_status(
        completion_port: isize,
        bytes_transferred: &mut u32,
        completion_key: usize,
        overlapped: &mut OVERLAPPED,
        timeout: Option<u32>,
    ) -> io::Result<u32> {
        let mut ul_num_entries_removed: u32 = 0;
        // can't coerce directly to *mut *mut usize and cant cast `&mut` as `*mut`
        // let completion_key_ptr: *mut &mut usize = completion_key_ptr;
        // // but we can cast a `*mut ...`
        // let completion_key_ptr: *mut *mut usize = completion_key_ptr as *mut *mut usize;
        let timeout = timeout.unwrap_or(INFINITE);
        let res = unsafe {
            GetQueuedCompletionStatus(
                completion_port,
                bytes_transferred,
                completion_key as *mut *mut usize,
                overlapped,
                timeout,
            )
        };

        if res == 0 {   
            
            
        Err(std::io::Error::last_os_error())
        } else {
            Ok(ul_num_entries_removed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_new_creates_valid_port() {
        let selector = Selector::new().expect("create completion port failed");
        assert!(selector.completion_port > 0);
    }

    #[test]
    fn selector_register() {
        let mut selector = Selector::new().expect("create completion port failed");
        let registrator = selector.registrator();
        let mut sock: TcpStream = TcpStream::connect("slowwly.robertomurray.co.uk:80").unwrap();
        let request = "GET /delay/1000/url/http://www.google.com HTTP/1.1\r\n\
                       Host: slowwly.robertomurray.co.uk\r\n\
                       Connection: close\r\n\
                       \r\n";
        sock.write_all(request.as_bytes())
            .expect("Error writing to stream");

        registrator
            .register(&mut sock, 1, Interests::readable())
            .expect("Error registering sock read event");
    }

    #[test]
    fn selector_select() {
        let mut selector = Selector::new().expect("create completion port failed");
        let registrator = selector.registrator();
        let mut sock: TcpStream = TcpStream::connect("slowwly.robertomurray.co.uk:80").unwrap();
        let request = "GET /delay/1000/url/http://www.google.com HTTP/1.1\r\n\
                       Host: slowwly.robertomurray.co.uk\r\n\
                       Connection: close\r\n\
                       \r\n";
        sock.write_all(request.as_bytes())
            .expect("Error writing to stream");
        
    
        registrator
            .register(&mut sock, 2, Interests::readable())
            .expect("Error registering sock read event");
        let mut entry = ffi::OVERLAPPED_ENTRY::zeroed();
        let mut events: Vec<ffi::OVERLAPPED_ENTRY> = vec![entry; 255];
        selector.select(&mut events).expect("Select failed");
       

        for event in events {
            let ol = unsafe {&*(event.lp_overlapped)};
            println!("EVT_OVERLAPPED {:?}", ol);
            println!("OVERLAPPED_STATUS {:?}", ol.internal as usize );
            println!("COMPL_KEY: {:?}", event.token().unwrap().value());
        }

        println!("SOCKET AFTER EVENT RETURN: {:?}", sock);

        let mut buffer = String::new();
        sock.read_to_string(&mut buffer).unwrap();

        println!("BUFFERS: {}", buffer);

    }
}
