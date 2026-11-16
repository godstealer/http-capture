import type {BodyCaptureState, BodyVariant, Flow, FlowRecord} from './types';

export const bodyVariants: readonly BodyVariant[] = ['request', 'response', 'originalRequest', 'originalResponse'];
type Context = Pick<FlowRecord, 'bodyCapture' | 'trafficOrigin'>;
/** Availability is independent of byte length and live receive completion.
 * Older Android LAN observations contain empty placeholders, not captured bytes. */
export function bodyCaptureState(flow: Context, variant: BodyVariant): BodyCaptureState {
  const states = flow.bodyCapture;
  if (states !== undefined && (states === null || typeof states !== 'object' || Array.isArray(states))) return 'notRecorded';
  if (states && Object.prototype.hasOwnProperty.call(states, variant)) return states[variant] === 'recorded' ? 'recorded' : 'notRecorded';
  return flow.trafficOrigin?.entry === 'android-lan' ? 'notRecorded' : 'recorded';
}
export function isBodyRecorded(flow: Context, variant: BodyVariant): boolean {
  return bodyCaptureState(flow, variant) === 'recorded';
}
export function assertBodyRecorded(flow: Context, variant: BodyVariant): void {
  if (!isBodyRecorded(flow, variant)) throw Error('Body was not recorded; this action requires captured bytes.');
}
export function assertRecordedBodies(flow: FlowRecord, variants: readonly BodyVariant[] = bodyVariants): void {
  for (const variant of variants) if (flow[variant]) assertBodyRecorded(flow, variant);
}
export function validateBodyCapture(flow: Context): void {
  if (flow.bodyCapture === undefined) return;
  if (!flow.bodyCapture || typeof flow.bodyCapture !== 'object' || Array.isArray(flow.bodyCapture)) throw Error('Invalid body capture metadata');
  for (const [variant, state] of Object.entries(flow.bodyCapture)) {
    if (!bodyVariants.includes(variant as BodyVariant) || !['recorded', 'notRecorded'].includes(state)) throw Error('Invalid body capture metadata');
  }
}

/** Copy a JSON-safe snapshot without claiming that compatibility placeholders
 * or observed prefixes are complete captured entities. */
export function normalizeFlowBodyCapture(flow: Flow): Flow {
  validateBodyCapture(flow);
  const missing=bodyVariants.filter(variant=>flow[variant]&&!isBodyRecorded(flow,variant));
  if(!missing.length)return flow;
  return {...flow,
    ...Object.fromEntries(missing.map(variant=>[variant,{...flow[variant],bodyBase64:''}])),
    bodyCapture:{...flow.bodyCapture,...Object.fromEntries(missing.map(variant=>[variant,'notRecorded']))},
  };
}
