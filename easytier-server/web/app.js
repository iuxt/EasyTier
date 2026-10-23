'use strict';
const $ = id => document.getElementById(id);
const labels = {pending:'等待审批',approved:'已同意',rejected:'已拒绝',revoked:'已撤销'};
const states = {waiting:'等待审批',running:'运行中',error:'运行失败',stopped:'已停止',offline:'失联停机'};
let devices = [], selected = null, logDevice = null, loading = false;
async function api(path, method='GET', body) {
 const response = await fetch(path,{method,headers:body ? {'Content-Type':'application/json'} : {},body:body ? JSON.stringify(body) : undefined});
 const data = await response.json();
 if (!response.ok) { if(response.status===401) showLogin(); throw new Error(data.error || '请求失败'); }
 return data;
}
function showLogin(){ $('login').hidden=false; $('console').hidden=true; $('logout').hidden=true; for(const id of ['editor','logDialog']) if($(id).open) $(id).close(); }
function showConsole(){ $('login').hidden=true; $('console').hidden=false; $('logout').hidden=false; }
function cell(text){ const td=document.createElement('td'); td.textContent=text; return td; }
function online(d){return Date.now()-new Date(d.last_seen).getTime()<20000;}
function render(){
 $('total').textContent=devices.length; $('pending').textContent=devices.filter(d=>d.status==='pending').length; $('online').textContent=devices.filter(online).length;
 $('devices').replaceChildren(); const filtered=devices.filter(d=>$('filter').value==='all'||d.status===$('filter').value); $('empty').hidden=filtered.length!==0;
 for(const d of filtered){const tr=document.createElement('tr');const name=cell(d.settings.hostname||d.hostname||'未命名设备');const small=document.createElement('small');small.textContent=d.id;name.append(small);tr.append(name);
 const status=cell('');const badge=document.createElement('span');badge.className='badge '+d.status;badge.textContent=labels[d.status]||d.status;status.append(badge);tr.append(status);
 const state=cell(online(d)?(states[d.runtime_status]||d.runtime_status):'离线');const version=document.createElement('small');version.textContent=`${d.platform} · 设置 ${d.applied_revision}/${d.revision}`;state.append(version);tr.append(state,cell(new Date(d.last_seen).toLocaleString()));
 const actions=cell('');for(const [label,fn] of [['设置 / 审批',()=>edit(d)],['日志',()=>showLogs(d)]]){const button=document.createElement('button');button.textContent=label;button.onclick=fn;actions.append(button);}tr.append(actions);$('devices').append(tr); }
}
async function refresh(silent=false){if(loading)return;loading=true;try{devices=await api('/api/admin/devices');showConsole();render();if(!silent)$('message').textContent='';}catch(e){if(!silent)$('message').textContent=e.message;}finally{loading=false;}}
function syncFields(){
 $('fixedIPField').hidden=$('autoIP').checked; $('ipv4').required=!$('autoIP').checked; $('ipv4').disabled=$('autoIP').checked;
 $('addressField').hidden=!$('allowInbound').checked;
}
function edit(d){
 selected=d;const settings=d.settings;
 $('deviceName').textContent=d.settings.hostname||d.hostname||d.id;$('deviceMeta').textContent=`${d.id} · ${d.platform} · ${d.version}`;
 $('hostname').value=settings.hostname;$('autoIP').checked=settings.auto_ip;$('ipv4').value=settings.ipv4;
 $('allowInbound').checked=settings.allow_inbound;$('publicAddress').value=settings.public_address;$('addressField').open=!!settings.public_address;
 $('enableIPv6').checked=settings.enable_ipv6;$('directConnection').checked=settings.direct_connection;$('udpHolePunch').checked=settings.udp_hole_punch;
 $('peerID').replaceChildren();
 for(const [value,label] of [['','自动选择已批准设备'],['none','不主动连接其他设备']]){const option=document.createElement('option');option.value=value;option.textContent=label;$('peerID').append(option);}
 const candidates=devices.filter(peer=>peer.id!==d.id&&peer.status==='approved');
 for(const peer of candidates){const option=document.createElement('option');option.value=peer.id;option.textContent=(peer.settings.hostname||peer.hostname||peer.id)+(peer.settings.allow_inbound?'':'（已关闭接入）');option.disabled=!peer.settings.allow_inbound;$('peerID').append(option);}
 if(settings.peer_id&&!['none',...candidates.map(peer=>peer.id)].includes(settings.peer_id)){const option=document.createElement('option');option.value=settings.peer_id;option.textContent='原接入节点不可用，请重新选择';$('peerID').append(option);}
 $('peerID').value=settings.peer_id;syncFields();$('runtimeError').textContent=d.runtime_error;$('editorMessage').textContent='';$('editor').showModal();
}
function readSettings(){return {hostname:$('hostname').value,auto_ip:$('autoIP').checked,ipv4:$('ipv4').value,allow_inbound:$('allowInbound').checked,public_address:$('publicAddress').value,enable_ipv6:$('enableIPv6').checked,direct_connection:$('directConnection').checked,udp_hole_punch:$('udpHolePunch').checked,peer_id:$('peerID').value};}
async function save(status,applyChanges=true){
 if(!selected)return;if(applyChanges&&!$('settingsForm').reportValidity())return;
 const buttons=['save','approve','reject','revoke'];buttons.forEach(id=>$(id).disabled=true);
 try{await api('/api/admin/devices/'+selected.id,'PUT',{status,...(applyChanges?{settings:readSettings()}:{}),revision:selected.revision});$('editor').close();await refresh();}
 catch(e){$('editorMessage').textContent=e.message;}finally{buttons.forEach(id=>$(id).disabled=false);}
}
async function loadLogs(){try{const entries=await api('/api/admin/devices/'+logDevice.id+'/logs');$('logs').textContent=entries.length?entries.map(e=>`${e.timestamp} ${e.level.padEnd(5)} ${e.target}: ${e.message}`).join('\n'):'暂无日志。';}catch(e){$('logs').textContent=e.message;}}
function showLogs(d){logDevice=d;$('logTitle').textContent=(d.settings.hostname||d.hostname||d.id)+' · 运行日志';$('logDialog').showModal();loadLogs();}
$('loginForm').onsubmit=async e=>{e.preventDefault();try{await api('/api/login','POST',{password:$('password').value});$('password').value='';await refresh();}catch(err){$('message').textContent=err.message;}};
$('logout').onclick=async()=>{try{await api('/api/logout','POST',{});showLogin();}catch(e){$('message').textContent=e.message;}};
$('refresh').onclick=()=>refresh();$('filter').onchange=render;$('closeEditor').onclick=()=>$('editor').close();$('closeLogs').onclick=()=>$('logDialog').close();$('reloadLogs').onclick=loadLogs;
$('approve').onclick=()=>save('approved');$('reject').onclick=()=>save('rejected',false);$('revoke').onclick=()=>save('revoked',false);$('save').onclick=()=>save(selected.status);
$('autoIP').onchange=syncFields;$('allowInbound').onchange=syncFields;$('settingsForm').onsubmit=e=>{e.preventDefault();save(selected.status);};
refresh(true);setInterval(()=>{if(!$('console').hidden)refresh(true);},5000);
