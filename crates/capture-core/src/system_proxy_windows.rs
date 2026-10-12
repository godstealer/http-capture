// WinINet per-connection options preserve manual proxy, PAC and auto-detect flags.
use super::*;
use std::{ffi::c_void, ptr};
#[repr(C)] #[derive(Clone,Copy)] union Value { flags:u32, text:*mut u16, time:[u32;2] }
#[repr(C)] #[derive(Clone,Copy)] struct OptionValue { option:u32, value:Value }
#[repr(C)] struct List { size:u32, connection:*mut u16, count:u32, error:u32, options:*mut OptionValue }
#[link(name="wininet")]
extern "system" {
    fn InternetQueryOptionW(handle:*mut c_void,option:u32,buffer:*mut c_void,size:*mut u32)->i32;
    fn InternetSetOptionW(handle:*mut c_void,option:u32,buffer:*mut c_void,size:u32)->i32;
}
#[link(name="kernel32")]
extern "system" { fn GlobalFree(memory:*mut c_void)->*mut c_void; }
pub fn supported()->bool{true}
pub fn target(_: &Settings,address:SocketAddr)->Result<Settings>{Ok(Settings{flags:3,server:format!("http={address};https={address}"),bypass:"<local>;localhost;127.*;[::1]".into(),pac:String::new(),gnome:None})}
fn wide(s:&str)->Vec<u16>{s.encode_utf16().chain(Some(0)).collect()}
fn check(ok:i32)->Result<()> {ensure!(ok!=0,"WinINet: {}",std::io::Error::last_os_error());Ok(())}
pub fn read()->Result<Settings>{unsafe{
    let mut options=[OptionValue{option:10,value:Value{time:[0,0]}},OptionValue{option:2,value:Value{time:[0,0]}},OptionValue{option:3,value:Value{time:[0,0]}},OptionValue{option:4,value:Value{time:[0,0]}}];
    let mut list=List{size:std::mem::size_of::<List>() as u32,connection:ptr::null_mut(),count:4,error:0,options:options.as_mut_ptr()};
    let mut size=list.size;
    let ok=InternetQueryOptionW(ptr::null_mut(),75,&mut list as *mut _ as *mut c_void,&mut size);
    let mut strings=Vec::new();
    for option in &options[1..] {
        let p=option.value.text;
        let s=if p.is_null(){String::new()}else{let mut len=0;while *p.add(len)!=0{len+=1;}let s=String::from_utf16_lossy(std::slice::from_raw_parts(p,len));GlobalFree(p as *mut c_void);s};
        strings.push(s);
    }
    check(ok)?;
    Ok(Settings{flags:options[0].value.flags,server:strings[0].clone(),bypass:strings[1].clone(),pac:strings[2].clone(),gnome:None})
}}
pub fn write(s:&Settings)->Result<()>{ensure!(s.gnome.is_none(),"Cannot restore GNOME settings on Windows");unsafe{
    let mut server=wide(&s.server);let mut bypass=wide(&s.bypass);let mut pac=wide(&s.pac);
    let mut options=[OptionValue{option:1,value:Value{flags:s.flags}},OptionValue{option:2,value:Value{text:server.as_mut_ptr()}},OptionValue{option:3,value:Value{text:bypass.as_mut_ptr()}},OptionValue{option:4,value:Value{text:pac.as_mut_ptr()}}];
    let mut list=List{size:std::mem::size_of::<List>() as u32,connection:ptr::null_mut(),count:4,error:0,options:options.as_mut_ptr()};
    check(InternetSetOptionW(ptr::null_mut(),75,&mut list as *mut _ as *mut c_void,list.size))?;
    check(InternetSetOptionW(ptr::null_mut(),39,ptr::null_mut(),0))?;
    check(InternetSetOptionW(ptr::null_mut(),37,ptr::null_mut(),0))
}}
