// Explicit keys, rather than a prefix scan, exclude skipped updates, tokens,
// browser state and unrelated future settings from portable configuration.
export const configurationPreferenceKeys=[
  'http-capture.theme','http-capture.language','http-capture.zoom','http-capture.listener.v1',
  'http-capture.body-editor.auto-format-paste.v1','http-capture.capture.columns.v1',
  'http-capture.capture.column-widths','http-capture.capture.id-sort',
  'http-capture.layout.inspector-direction','http-capture.layout.composer-columns',
  'http-capture.layout.capture-list','http-capture.layout.capture-messages-row',
  'http-capture.layout.capture-messages-column','http-capture.code-target',
  'http-capture.clienthello-templates.v1','http-capture.grpc.proto.v1',
  'http-capture.request-modes',
] as const;
export function readConfigurationPreferences(storage:Pick<Storage,'getItem'>=localStorage):Record<string,string>{
  const values:Record<string,string>={};
  for(const key of configurationPreferenceKeys){const value=storage.getItem(key);if(value!==null)values[key]=value}
  return values;
}
export function applyConfigurationPreferences(values:Record<string,string>,storage:Pick<Storage,'getItem'|'setItem'|'removeItem'>=localStorage){
  const keys=Object.keys(values);
  if(keys.some(key=>!configurationPreferenceKeys.includes(key as typeof configurationPreferenceKeys[number])||typeof values[key]!=='string'))throw Error('Invalid configuration preferences');
  const before=new Map(configurationPreferenceKeys.map(key=>[key,storage.getItem(key)]));
  try{
    // A selected preferences section replaces that category, including defaults
    // for keys absent on the source device.
    const optional=['http-capture.request-modes','http-capture.grpc.proto.v1','http-capture.clienthello-templates.v1'];
    for(const key of configurationPreferenceKeys){if(Object.hasOwn(values,key))storage.setItem(key,values[key]);else if(!optional.includes(key))storage.removeItem(key)}
  }catch(error){
    for(const [key,value] of before){try{if(value===null)storage.removeItem(key);else storage.setItem(key,value)}catch{/* The caller retains values for retry before restart. */}}
    throw error;
  }
}
