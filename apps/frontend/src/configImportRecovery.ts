import {applyConfigurationPreferences} from './configPreferences';

export const configurationImportAckKey='http-capture.config-import-epoch.v1';
export interface ConfigurationImportStatus {
  epoch?:string;
  frozen:boolean;
  importedSections:string[];
  preferences?:Record<string,string>;
  warnings?:string[];
}
interface ImportPreferences {epoch?:string;importedSections:string[];preferences?:Record<string,string>}
type PreferenceStorage=Pick<Storage,'getItem'|'setItem'|'removeItem'>;

/** The database retains the last committed import until this browser origin has
 * projected it. An acknowledgement never travels in a portable config file.
 * A missing response or process crash therefore cannot silently lose imported
 * preferences, while later user changes survive refresh/restart. */
export function acknowledgeConfigurationImport(result:ImportPreferences,storage?:PreferenceStorage):boolean {
  if(!result.importedSections.includes('preferences'))return false;
  if(typeof result.epoch!=='string'||!result.epoch||result.epoch.length>128)throw Error('Missing configuration import epoch');
  const target=storage??localStorage;
  if(target.getItem(configurationImportAckKey)===result.epoch)return false;
  if(!result.preferences||typeof result.preferences!=='object'||Array.isArray(result.preferences))throw Error('Missing imported interface preferences');
  applyConfigurationPreferences(result.preferences,target);
  // Write acknowledgement last. If it fails, keep the startup/import retry
  // screen; the same immutable preferences can safely be applied again.
  target.setItem(configurationImportAckKey,result.epoch);
  return true;
}

export async function recoverConfigurationStartup(readStatus:()=>Promise<ConfigurationImportStatus>,storage?:PreferenceStorage,isCurrent:()=>boolean=()=>true):Promise<ConfigurationImportStatus|undefined>{
  let status:ConfigurationImportStatus;
  try{status=await readStatus()}catch{return undefined} // Older cores/offline previews still load normally.
  if(!isCurrent())return undefined;
  if(!status||typeof status.frozen!=='boolean'||!Array.isArray(status.importedSections)||!status.importedSections.every(section=>typeof section==='string'))throw Error('Invalid configuration import status');
  acknowledgeConfigurationImport(status,storage);
  return status;
}
