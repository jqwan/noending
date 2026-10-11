import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import type { ProjectCardData, Workstream, WorkstreamProject } from "../../types";

/** Projects are explicit memberships; editing a task never expands directories. */
export default function WorkstreamFormModal({ onClose,onCreated,onSaved,workstream,projects: initialProjects=[],initialProjectId }: {
  onClose: () => void; onCreated?: (w:Workstream)=>void; onSaved?:()=>void;
  workstream?:Workstream; projects?:WorkstreamProject[]; initialProjectId?:string;
}) {
  const [title,setTitle]=useState(workstream?.title ?? "");
  const [description,setDescription]=useState(workstream?.description ?? "");
  const [projects,setProjects]=useState<ProjectCardData[]>([]);
  const [selected,setSelected]=useState<string[]>(()=>workstream ? initialProjects.map(p=>p.id) : initialProjectId && initialProjectId!=="none" ? [initialProjectId]:[]);
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState("");
  const pending=useRef(false);
  useEffect(()=>{ let active=true; api.listProjectCards().then(p=>{if(active)setProjects(p);}).catch(e=>{if(active)setError(String(e));});return()=>{active=false;}; },[]);
  const browse=async()=>{
    if(pending.current)return;
    pending.current=true;setBusy(true);setError("");
    try {
      const path=await open({directory:true,multiple:false,title:"选择项目目录"});
      if(!path)return;
      const result=await api.addProjectPath(Array.isArray(path)?path[0]:path);
      setProjects(await api.listProjectCards());
      if(result.project_id)setSelected(ids=>[...new Set([...ids,result.project_id!])]);
    }catch(e){setError(String(e));}finally{pending.current=false;setBusy(false);}
  };
  const save=async()=>{
    if(pending.current || !title.trim())return;
    pending.current=true;setBusy(true);setError("");
    try {
      if(workstream){
        await api.updateWorkstream({...workstream,title:title.trim(),description:description.trim()});
        await api.setWorkstreamProjects(workstream.id,selected);
        onSaved?.();
      }else{ const created=await api.createWorkstream(title.trim(),description.trim(),selected); onCreated?.(created); }
      onClose();
    }catch(e){setError(String(e));}finally{pending.current=false;setBusy(false);}
  };
  return <Modal title={workstream?"编辑任务":"新建任务"} onClose={()=>{if(!busy)onClose();}}>
    <form onSubmit={e=>{e.preventDefault();void save();}}>
      <label className="field"><span>标题</span><input type="text" autoFocus value={title} onChange={e=>setTitle(e.target.value)} disabled={busy}/></label>
      <label className="field"><span>描述</span><textarea value={description} onChange={e=>setDescription(e.target.value)} disabled={busy}/></label>
      <div className="field"><span>关联项目（可选，可多选）</span>
        <div className="workstream-project-chips">{selected.map(id=>{
          const project=projects.find(p=>p.id===id) ?? initialProjects.find(p=>p.id===id);
          return <div className="workstream-project-chip" key={id}><Icon name="project"/><span>{project?.name ?? id}</span><button type="button" className="workstream-project-chip-remove" disabled={busy} aria-label={`移除项目 ${project?.name ?? id}`} onClick={()=>setSelected(ids=>ids.filter(p=>p!==id))}><Icon name="close"/></button></div>;
        })}</div>
        <div className="row"><select aria-label="关联项目选择" disabled={busy} value="" onChange={e=>{const id=e.target.value;if(id)setSelected(ids=>[...ids,id]);}}>
          <option value="">+ 添加关联项目…</option>{projects.filter(p=>!selected.includes(p.id)).map(p=><option key={p.id} value={p.id}>{p.name}</option>)}
        </select><button type="button" className="btn" style={{flexShrink:0}} disabled={busy} onClick={()=>void browse()}>新增目录…</button></div>
      </div>
      {error && <p role="alert" className="warning">{error}</p>}
      <div className="row" style={{justifyContent:"flex-end"}}><button type="button" className="btn" disabled={busy} onClick={onClose}>取消</button><button type="submit" className="btn primary" disabled={busy || !title.trim()}>{busy?"保存中…":workstream?"保存":"创建"}</button></div>
    </form>
  </Modal>;
}
