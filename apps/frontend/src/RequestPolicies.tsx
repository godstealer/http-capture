import { useId, useState } from 'react';
import RequestBlocking from './RequestBlocking';
import FaultRulesPage from './FaultRulesPage';
import { t } from './i18n';
import './fault-rules.css';

export default function RequestPolicies(props:{connected:boolean;onSaved?:()=>Promise<void>}) {
  const [tab,setTab]=useState<'blocking'|'faults'>('blocking');
  const id=useId();
  function select(value:'blocking'|'faults'){setTab(value);document.getElementById(`${id}-${value}`)?.focus()}
  return <div className="request-policies">
    <div className="request-policy-tabs" role="tablist" aria-label={t('请求规则')} onKeyDown={event=>{if(['ArrowLeft','ArrowRight','Home','End'].includes(event.key)){event.preventDefault();select(event.key==='Home'?'blocking':event.key==='End'?'faults':tab==='blocking'?'faults':'blocking')}}}>
      {(['blocking','faults'] as const).map(value=><button type="button" key={value} id={`${id}-${value}`} role="tab" aria-selected={tab===value} aria-controls={`${id}-${value}-panel`} tabIndex={tab===value?0:-1} onClick={()=>setTab(value)}>{t(value==='blocking'?'阻止请求':'故障模拟')}</button>)}
    </div>
    <div className="request-policy-panel" id={`${id}-blocking-panel`} role="tabpanel" aria-labelledby={`${id}-blocking`} hidden={tab!=='blocking'}><RequestBlocking {...props}/></div>
    <div className="request-policy-panel" id={`${id}-faults-panel`} role="tabpanel" aria-labelledby={`${id}-faults`} hidden={tab!=='faults'}><FaultRulesPage connected={props.connected}/></div>
  </div>;
}
