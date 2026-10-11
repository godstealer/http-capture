use anyhow::{ensure, Result};
use serde::Serialize;
use std::{io::BufReader, path::Path};
#[derive(Debug, Serialize)]
#[serde(rename_all="camelCase")]
pub struct Status {pub supported:bool,pub installed:Option<bool>,pub scope:String,pub certificate:crate::model::CertificateDetails}
fn der(path:&Path)->Result<Vec<u8>> {
    let mut reader=BufReader::new(std::fs::File::open(path)?);
    let cert=rustls_pemfile::certs(&mut reader).next().transpose()?.ok_or_else(||anyhow::anyhow!("CA certificate is missing"))?;
    Ok(cert.to_vec())
}
pub fn status(path:&Path)->Result<Status>{
    let bytes=der(path)?;let certificate=crate::tls_details::certificates(&[bytes.clone().into()]).remove(0);
    Ok(Status{supported:cfg!(windows),installed:platform::installed(&bytes)?,scope:if cfg!(windows){"Windows CurrentUser\\Root"}else{"Manual installation required"}.into(),certificate})
}
pub fn change(path:&Path,install:bool,sha256:&str)->Result<Status>{
    let bytes=der(path)?;
    let certificate=crate::tls_details::certificates(&[bytes.clone().into()]).remove(0);
    ensure!(certificate.sha256==sha256,"CA changed; refresh certificate details before continuing");
    platform::change(&bytes,install)?;
    let result=status(path)?;ensure!(result.installed==Some(install),"Certificate store change could not be verified");Ok(result)
}
#[cfg(not(windows))] mod platform {
    use super::*;
    pub fn installed(_: &[u8])->Result<Option<bool>>{Ok(None)}
    pub fn change(_: &[u8],_:bool)->Result<()>{anyhow::bail!("Automatic CA trust management is currently available on Windows only; export and install the CA manually")}
}
#[cfg(windows)] mod platform {
    use super::*;use std::{ffi::c_void,ptr};
    #[repr(C)] struct Cert {encoding:u32,bytes:*const u8,len:u32,info:*const c_void,store:*mut c_void}
    #[link(name="crypt32")] extern "system" {
        fn CertOpenStore(provider:*const u8,encoding:u32,crypt:*mut c_void,flags:u32,parameter:*const c_void)->*mut c_void;
        fn CertCloseStore(store:*mut c_void,flags:u32)->i32;
        fn CertEnumCertificatesInStore(store:*mut c_void,previous:*const Cert)->*const Cert;
        fn CertFreeCertificateContext(cert:*const Cert)->i32;
        fn CertAddEncodedCertificateToStore(store:*mut c_void,encoding:u32,bytes:*const u8,len:u32,disposition:u32,result:*mut *const Cert)->i32;
        fn CertDeleteCertificateFromStore(cert:*const Cert)->i32;
    }
    struct Store(*mut c_void);impl Drop for Store{fn drop(&mut self){unsafe{CertCloseStore(self.0,0);}}}
    fn open(write:bool)->Result<Store>{
        let root:Vec<u16>="Root".encode_utf16().chain(Some(0)).collect();
        let handle=unsafe{CertOpenStore(10usize as *const u8,0,ptr::null_mut(),0x10000|0x4000|if write{0}else{0x8000},root.as_ptr() as *const c_void)};
        ensure!(!handle.is_null(),"Open current-user root store: {}",std::io::Error::last_os_error());Ok(Store(handle))
    }
    // Compare the complete DER bytes, never the shared CA display name.
    fn find(store:&Store,bytes:&[u8])->Result<Option<*const Cert>>{unsafe{
        let mut cert=ptr::null();loop{cert=CertEnumCertificatesInStore(store.0,cert);if cert.is_null(){
            let error=std::io::Error::last_os_error();ensure!(error.raw_os_error()==Some(0x80092004u32 as i32),"Enumerate root store: {error}");return Ok(None);
        }if std::slice::from_raw_parts((*cert).bytes,(*cert).len as usize)==bytes{return Ok(Some(cert));}}
    }}
    pub fn installed(bytes:&[u8])->Result<Option<bool>>{let store=open(false)?;let found=find(&store,bytes)?;if let Some(cert)=found{unsafe{CertFreeCertificateContext(cert);}}Ok(Some(found.is_some()))}
    pub fn change(bytes:&[u8],install:bool)->Result<()>{
        let store=open(true)?;let found=find(&store,bytes)?;
        unsafe {match (install,found){
            (true,Some(cert))=>{CertFreeCertificateContext(cert);},
            (true,None)=>{ensure!(CertAddEncodedCertificateToStore(store.0,1,bytes.as_ptr(),bytes.len() as u32,1,ptr::null_mut())!=0,"Install CA: {}",std::io::Error::last_os_error());},
            (false,Some(cert))=>{ensure!(CertDeleteCertificateFromStore(cert)!=0,"Remove CA: {}",std::io::Error::last_os_error());},
            (false,None)=>{}
        }}Ok(())
    }
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn rejects_stale_fingerprint_before_store_access(){let dir=tempfile::tempdir().unwrap();let ca=crate::ca::CertificateAuthority::load_or_create(dir.path()).unwrap();assert!(change(&ca.cert_path,true,"stale").unwrap_err().to_string().contains("CA changed"));}
    #[cfg(windows)] #[test] fn windows_ca_status_read_only(){let dir=tempfile::tempdir().unwrap();let ca=crate::ca::CertificateAuthority::load_or_create(dir.path()).unwrap();let state=status(&ca.cert_path).unwrap();assert_eq!(state.installed,Some(false));assert_eq!(state.certificate.sha256.len(),95);}

}
