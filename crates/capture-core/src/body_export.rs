//! Decode directly to a new file. Never leave a partial export on failure.
use anyhow::{ensure,Result};
use base64::{engine::general_purpose::STANDARD,read::DecoderReader};
use std::{fs::OpenOptions,io::{Cursor,Read},path::Path};
pub const MAX_EXPORT_BYTES:u64=256*1024*1024;
pub fn save_base64(path:&Path,body:&str)->Result<u64>{
    ensure!(body.len() as u64 <= MAX_EXPORT_BYTES.div_ceil(3)*4,"Export exceeds 256 MiB limit");
    let mut options=OpenOptions::new();options.write(true).create_new(true);
    #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
    let mut file=options.open(path)?;
    let result=(||{
        let mut decoder=DecoderReader::new(Cursor::new(body.as_bytes()),&STANDARD).take(MAX_EXPORT_BYTES+1);
        let size=std::io::copy(&mut decoder,&mut file)?;
        ensure!(size<=MAX_EXPORT_BYTES,"Export exceeds 256 MiB limit");
        file.sync_all()?;Ok(size)
    })();
    drop(file);
    if result.is_err(){std::fs::remove_file(path)?;}
    result
}
#[cfg(test)]mod tests{
    use super::*;use base64::Engine;
    #[test]fn exports_large_binary_exactly(){let dir=tempfile::tempdir().unwrap();let path=dir.path().join("body.bin");let bytes:Vec<u8>=(0..9*1024*1024).map(|n|(n%251) as u8).collect();assert_eq!(save_base64(&path,&STANDARD.encode(&bytes)).unwrap(),bytes.len() as u64);assert_eq!(std::fs::read(&path).unwrap(),bytes);assert!(save_base64(&path,"YQ==").is_err());assert_eq!(std::fs::metadata(path).unwrap().len(),bytes.len() as u64);}
    #[test]fn malformed_export_removes_partial_file(){let dir=tempfile::tempdir().unwrap();let path=dir.path().join("body.bin");let mut body=STANDARD.encode(vec![1;100000]);body.push('!');assert!(save_base64(&path,&body).is_err());assert!(!path.exists());assert_eq!(save_base64(&path,"").unwrap(),0);}
}
