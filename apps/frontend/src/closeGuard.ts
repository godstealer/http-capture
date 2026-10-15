export type CloseFailure = 'dirty-library' | 'save-failed' | 'update-in-progress';
interface Actions {
  blocked?:()=>boolean;
  dirty:()=>boolean;
  flush:()=>Promise<boolean>;
  destroy:()=>Promise<void>;
  error:(error:unknown)=>void;
  busy:(value:boolean)=>void;
}
// Owns one close attempt; repeated native close events share the same operation.
export function createCloseGuard(actions:Actions) {
  let pending:Promise<void>|undefined;
  let disposed=false;
  return {
    close():Promise<void> {
      if(disposed)return Promise.resolve();
      if(pending)return pending;
      actions.busy(true);
      pending=Promise.resolve().then(async()=>{
        if(disposed)return;
        if(actions.blocked?.())throw 'update-in-progress' satisfies CloseFailure;
        if(actions.dirty())throw 'dirty-library' satisfies CloseFailure;
        if(!await actions.flush())throw 'save-failed' satisfies CloseFailure;
        if(disposed)return;
        if(actions.blocked?.())throw 'update-in-progress' satisfies CloseFailure;
        if(actions.dirty())throw 'dirty-library' satisfies CloseFailure;
        await actions.destroy();
      }).catch(error=>{if(!disposed)actions.error(error);}).finally(()=>{
        pending=undefined;if(!disposed)actions.busy(false);
      });
      return pending;
    },
    dispose(){disposed=true;},
  };
}
