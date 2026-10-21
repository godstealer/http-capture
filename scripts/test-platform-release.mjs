import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {generateKeyPairSync,randomBytes,createHash,sign} from 'node:crypto';
import {collectRelease,sha256} from './collect-release.mjs';
import {platforms,projectRoot} from './release-updater.mjs';
import {verifyPlatformRelease} from './verify-platform-release.mjs';
const version='0.1.34',sourceSha='a'.repeat(40);
function fixture(t){
 const parent=path.join(projectRoot,'.local');fs.mkdirSync(parent,{recursive:true});const root=fs.mkdtempSync(path.join(parent,'staged-release-test-'));
 t.after(()=>{assert.equal(path.dirname(path.resolve(root)),path.resolve(parent));assert(path.basename(root).startsWith('staged-release-test-'));fs.rmSync(root,{recursive:true,force:true})});
 const directory=path.join(root,'assets');fs.mkdirSync(directory);const{publicKey,privateKey}=generateKeyPairSync('ed25519'),id=randomBytes(8);
 const key=Buffer.from(`untrusted comment: disposable test public key\n${Buffer.concat([Buffer.from('Ed'),id,publicKey.export({format:'der',type:'spki'}).subarray(-32)]).toString('base64')}\n`).toString('base64');
 const metadata={};for(const label of ['windows-x64','linux-x64']){
  const p=platforms[label],bundle=path.join(root,label);fs.mkdirSync(bundle);
  for(const suffix of new Set([...p.installers,p.updater,...(p.additionalUpdaters??[]).map(x=>x.suffix)]))fs.writeFileSync(path.join(bundle,`HTTP-Capture_${version}${suffix}`),`${label}:${suffix}`);
  for(const suffix of [p.updater,...(p.additionalUpdaters??[]).map(x=>x.suffix)]){
   const file=path.join(bundle,`HTTP-Capture_${version}${suffix}`),signature=sign(null,createHash('blake2b512').update(fs.readFileSync(file)).digest(),privateKey),comment=`timestamp:1\tfile:${path.basename(file)}\tversion:${version}`;
   const packet=Buffer.concat([Buffer.from('ED'),id,signature]),global=sign(null,Buffer.concat([signature,Buffer.from(comment)]),privateKey);
   fs.writeFileSync(`${file}.sig`,Buffer.from(`untrusted comment: disposable test signature\n${packet.toString('base64')}\ntrusted comment: ${comment}\n${global.toString('base64')}\n`).toString('base64'));
  }
  metadata[label]=collectRelease({label,bundleDirectory:bundle,outputDirectory:directory,version,sourceSha,updaterPublicKey:key,portable:false,runtime:{platform:p.os,arch:p.arch}});
 }
 const portable=`linux-x64-HTTP-Capture_${version}-portable.tar.gz`;fs.writeFileSync(path.join(directory,portable),'test portable');fs.appendFileSync(path.join(directory,'SHA256SUMS-linux-x64.txt'),`${sha256(path.join(directory,portable))}  ${portable}\n`);
 const options={directory,version,sourceSha,group:'windows-linux'},trust={publicKey:key};
 function rehash(name){for(const label of ['windows-x64','linux-x64']){const file=path.join(directory,`SHA256SUMS-${label}.txt`),raw=fs.readFileSync(file,'utf8');fs.writeFileSync(file,raw.split('\n').map(line=>line.endsWith(`  ${name}`)?`${sha256(path.join(directory,name))}  ${name}`:line).join('\n'))}}
 return{directory,options,trust,metadata,rehash};
}
test('staged Windows/Linux verifies complete inventory, all checksums and three real signatures without a global manifest',async t=>{const f=fixture(t),report=await verifyPlatformRelease(f.options,f.trust);assert.equal(report.assetCount,11);assert.equal(report.checksumCount,9);assert.equal(report.signaturesVerified,3);assert.equal(report.globalManifest,false)});
test('staged packages remain cryptographically protected after metadata and checksums are forged',async t=>{const f=fixture(t),meta=f.metadata['windows-x64'];fs.appendFileSync(path.join(f.directory,meta.file),'changed');meta.sha256=sha256(path.join(f.directory,meta.file));const metadata='updater-windows-x64.json';fs.writeFileSync(path.join(f.directory,metadata),JSON.stringify(meta));f.rehash(meta.file);f.rehash(metadata);await assert.rejects(verifyPlatformRelease(f.options,f.trust),/Package signature failed/)});
test('staged release rejects wrong identities, absent Mac, unsafe groups and an incomplete global manifest',async t=>{const f=fixture(t);await assert.rejects(verifyPlatformRelease({...f.options,sourceSha:'b'.repeat(40)},f.trust),/identity/);await assert.rejects(verifyPlatformRelease({...f.options,version:'0.1.35'},f.trust),/identity/);await assert.rejects(verifyPlatformRelease({...f.options,group:'__proto__'},f.trust),/group/);await assert.rejects(verifyPlatformRelease({...f.options,group:'macos'},f.trust));fs.writeFileSync(path.join(f.directory,'latest.json'),'{}');await assert.rejects(verifyPlatformRelease(f.options,f.trust),/unexpected artifacts/)});
test('staged checksums cannot omit a file or contain duplicate targets',async t=>{const f=fixture(t),file=path.join(f.directory,'SHA256SUMS-windows-x64.txt'),original=fs.readFileSync(file,'utf8');fs.writeFileSync(file,original.split('\n').slice(1).join('\n'));await assert.rejects(verifyPlatformRelease(f.options,f.trust),/coverage/);fs.writeFileSync(file,original+original);await assert.rejects(verifyPlatformRelease(f.options,f.trust),/checksum target/)});
