import {createReadStream,lstatSync,readFileSync,readdirSync} from 'node:fs';
import {createHash,createPublicKey,verify} from 'node:crypto';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {parseArgs,isDeepStrictEqual} from 'node:util';
import {assetPath,assertSourceSha,assertVersion,platforms,signatureContent,publicKey} from './release-updater.mjs';
import {committedReleaseTrust} from './verify-published-release.mjs';

const groups={'windows-linux':['windows-x64','linux-x64'],macos:['macos-arm64','macos-x64']};
const requireCondition=(condition,message)=>{if(!condition)throw Error(message)};
async function digest(file,algorithm='sha256'){const hash=createHash(algorithm);for await(const chunk of createReadStream(file))hash.update(chunk);return hash.digest()}

// A staged release deliberately has no global update manifest. This validates
// only the requested complete platform group, never authorizes channel writes.
export async function verifyPlatformRelease({directory,version,sourceSha,group},trust){
 assertVersion(version);assertSourceSha(sourceSha);requireCondition(Object.hasOwn(groups,group),'Unknown platform group');
 const labels=groups[group],root=path.resolve(directory),key=publicKey({plugins:{updater:{pubkey:(trust??committedReleaseTrust(sourceSha)).publicKey}}});
 const names=readdirSync(root);for(const name of names){assetPath(root,name);const stat=lstatSync(assetPath(root,name));requireCondition(stat.isFile()&&!stat.isSymbolicLink()&&stat.size>0,`Invalid artifact ${name}`)}
 const expected=new Set(),updaters=[];
 const one=(label,suffix)=>{const matches=names.filter(name=>name.startsWith(`${label}-`)&&name.endsWith(suffix));requireCondition(matches.length===1,`Expected one ${label} ${suffix}`);expected.add(matches[0]);return matches[0]};
 for(const label of labels){
  const platform=platforms[label],metadataName=`updater-${label}.json`;expected.add(metadataName);expected.add(`SHA256SUMS-${label}.txt`);
  const metadata=JSON.parse(readFileSync(assetPath(root,metadataName),'utf8'));
  requireCondition(metadata.schemaVersion===1&&metadata.label===label&&metadata.target===platform.target&&metadata.version===version&&metadata.sourceSha===sourceSha,`Wrong build identity: ${label}`);
  requireCondition(isDeepStrictEqual(Object.keys(metadata.extraUpdaters??{}).sort(),(platform.additionalUpdaters??[]).map(item=>item.target).sort()),`Wrong updater targets: ${label}`);
  for(const suffix of platform.installers)one(label,suffix);if(label==='linux-x64')one(label,'-portable.tar.gz');
  for(const [suffix,artifact] of [[platform.updater,metadata],...(platform.additionalUpdaters??[]).map(item=>[item.suffix,metadata.extraUpdaters[item.target]])]){
   requireCondition(artifact.file===one(label,suffix)&&artifact.signatureFile===`${artifact.file}.sig`,`Wrong artifact path: ${label}`);
   expected.add(artifact.signatureFile);requireCondition((await digest(assetPath(root,artifact.file))).toString('hex')===artifact.sha256,`Metadata SHA256 mismatch: ${artifact.file}`);updaters.push(artifact);
  }
 }
 requireCondition(isDeepStrictEqual([...expected].sort(),names.sort()),'Staged release contains missing or unexpected artifacts (no global manifest allowed)');
 const covered=new Set();for(const label of labels){
  const checksum=`SHA256SUMS-${label}.txt`;
  for(const line of readFileSync(assetPath(root,checksum),'utf8').trim().split(/\r?\n/)){
   const match=/^([a-f0-9]{64})  ([A-Za-z0-9][A-Za-z0-9._-]*)$/i.exec(line);requireCondition(match,`Invalid checksum line: ${checksum}`);const[,hash,name]=match;
   requireCondition(expected.has(name)&&!name.startsWith('SHA256SUMS-')&&(name.startsWith(`${label}-`)||name===`updater-${label}.json`)&&!covered.has(name),`Invalid checksum target: ${name}`);covered.add(name);
   requireCondition((await digest(assetPath(root,name))).toString('hex')===hash.toLowerCase(),`SHA256 mismatch: ${name}`);
  }
 }
 requireCondition(isDeepStrictEqual([...covered].sort(),names.filter(name=>!name.startsWith('SHA256SUMS-')).sort()),'Incomplete checksum coverage');
 const keyPacket=Buffer.from(Buffer.from(key,'base64').toString('utf8').trim().split(/\r?\n/)[1],'base64');requireCondition(keyPacket.subarray(0,2).toString('ascii')==='Ed','Unsupported public key algorithm');
 const verificationKey=createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),keyPacket.subarray(10)]),format:'der',type:'spki'});
 for(const artifact of updaters){
  const content=signatureContent(readFileSync(assetPath(root,artifact.signatureFile),'utf8'),version,key),lines=Buffer.from(content,'base64').toString('utf8').trim().split(/\r?\n/),packet=Buffer.from(lines[1],'base64');
  requireCondition(packet.subarray(0,2).toString('ascii')==='ED','Unsupported signature algorithm');const signature=packet.subarray(10),comment=Buffer.from(lines[2].slice('trusted comment: '.length),'utf8');
  requireCondition(verify(null,Buffer.concat([signature,comment]),verificationKey,Buffer.from(lines[3],'base64')),`Comment signature failed: ${artifact.file}`);
  requireCondition(verify(null,await digest(assetPath(root,artifact.file),'blake2b512'),verificationKey,signature),`Package signature failed: ${artifact.file}`);
 }
 return{version,sourceSha,group,platforms:labels,assetCount:names.length,checksumCount:covered.size,signaturesVerified:updaters.length,globalManifest:false};
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)){
 try{const{values}=parseArgs({options:{directory:{type:'string'},version:{type:'string'},'source-sha':{type:'string'},group:{type:'string'}}});console.log(JSON.stringify(await verifyPlatformRelease({directory:values.directory,version:values.version,sourceSha:values['source-sha'],group:values.group}),null,2))}catch(error){console.error(error.message);process.exitCode=1}
}
