import { useEffect, useState } from "react";
import { Glyph } from "../../components/Glyph";
import { SelectField } from "../../components/SelectField";
import { api, type SubscriptionKind, type SubscriptionStatus } from "../../api";
import { errorText } from "../../utils";

/// 添加 / 编辑订阅套餐弹窗：套餐类型决定凭据字段与 OAuth 流程。
export function PlanFormModal({ form, onClose, onSaved }: {
  form: { mode: "add" } | { mode: "edit"; status: SubscriptionStatus };
  onClose: () => void;
  onSaved: () => void;
}) {
  const editing = form.mode === "edit" ? form.status : null;
  const [kinds, setKinds] = useState<SubscriptionKind[]>([]);
  const [kind, setKind] = useState(editing?.provider ?? "glm");
  const [name, setName] = useState(editing?.title ?? "");
  const [nameTouched, setNameTouched] = useState(editing !== null);
  const [platform, setPlatform] = useState(editing?.platform ?? "zai");
  const [baseUrl, setBaseUrl] = useState(editing?.baseUrl ?? "");
  const [key, setKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const selected = kinds.find((entry) => entry.id === kind);
  const isOAuth = selected?.auth === "oauth";
  // 登录型供应商的授权文案：Google OAuth 与 Cursor 浏览器登录流程不同源。
  const oauthCopy = kind === "cursor"
    ? {
        hint: "点击后打开浏览器完成 Cursor 登录；登录 token 约两个月有效，过期后删除该套餐重新登录即可，凭据只保存在本机。",
        action: "通过 Cursor 登录",
        waiting: "等待浏览器登录…",
      }
    : {
        hint: "点击后打开浏览器完成 Google 授权；需要 Google AI Pro / Ultra 订阅的账号，凭据只保存在本机。",
        action: "通过 Google 登录",
        waiting: "等待浏览器授权…",
      };
  // Catalog from the backend drives the kind picker and the credential
  // fields; the name follows the picked kind until the user edits it.
  useEffect(() => {
    let cancelled = false;
    api.listSubscriptionKinds().then((list) => {
      if (cancelled || list.length === 0) return;
      setKinds(list);
      if (!editing) {
        setKind(list[0].id);
        setName((current) => current || list[0].title);
      }
    }).catch(() => { /* keep the glm default; add validates server-side */ });
    return () => { cancelled = true; };
  }, [editing]);
  const needsUrl = !!selected?.urlLabel;
  const submit = async () => {
    if (saving || !name.trim() || (!isOAuth && !editing && !key.trim()) || (needsUrl && !baseUrl.trim())) return;
    setSaving(true);
    setError("");
    try {
      if (isOAuth) {
        // 登录型套餐：添加走浏览器授权（凭据只在后端落地），编辑只改名。
        if (editing) await api.updateSubscriptionPlan(editing.id, name.trim(), "", "", "");
        else if (kind === "cursor") await api.cursorLogin(name.trim());
        else await api.antigravityLogin(name.trim());
      } else if (editing) {
        await api.updateSubscriptionPlan(editing.id, name.trim(), platform, key.trim(), baseUrl.trim());
      } else {
        await api.addSubscriptionPlan(kind, name.trim(), platform, key.trim(), baseUrl.trim());
      }
      onSaved();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setSaving(false);
    }
  };
  return <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
    <div className="modal" role="dialog" aria-modal="true" aria-labelledby="plan-form-title">
      <div className="modal-head">
        <div><div className="eyebrow">订阅与配额</div><h2 id="plan-form-title">{editing ? "编辑订阅套餐" : "添加订阅套餐"}</h2></div>
        <button className="icon-button" aria-label="关闭" onClick={onClose}><Glyph name="close" size={20} /></button>
      </div>
      <form className="modal-body" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
        <small className="form-hint">同一个订阅可以添加多份，用名称区分；凭据只保存在本机。</small>
        <div className="subscription-form">
          {!editing && kinds.length > 0 && <label>套餐类型
            <SelectField label="套餐类型" value={kind} options={kinds.map((entry) => ({ value: entry.id, label: entry.title }))} onChange={(next) => {
              const entry = kinds.find((item) => item.id === next);
              setKind(next);
              if (entry) {
                if (!nameTouched) setName(entry.title);
                setPlatform(entry.platforms[0]?.[0] ?? "");
                setBaseUrl("");
                // 凭据不跨套餐类型复用：换类型即丢弃已输入的 Key。
                setKey("");
              }
            }} />
          </label>}
          <label>名称
            <input value={name} maxLength={100} placeholder="例如：主力账号" autoFocus onChange={(event) => { setNameTouched(true); setName(event.target.value); }} />
          </label>
          {/* 平台与凭据字段随所选套餐类型切换 */}
          {(selected?.platforms.length ?? 0) > 0 && <label>平台
            <SelectField label="平台" value={platform} options={(selected?.platforms ?? []).map(([optionValue, optionLabel]) => ({ value: optionValue, label: optionLabel }))} onChange={setPlatform} />
          </label>}
          {needsUrl && <label>{selected?.urlLabel}
            <input value={baseUrl} placeholder={selected?.urlPlaceholder ?? "https://…"} onChange={(event) => setBaseUrl(event.target.value)} />
          </label>}
          {!isOAuth && <label>{selected?.keyLabel ?? "凭据 Key"}
            <input type="password" value={key} autoComplete="off" placeholder={editing ? `留空则保留现有 Key（${editing.keyHint ?? "已保存"}）` : selected?.keyPlaceholder ?? "粘贴凭据 Key"} onChange={(event) => setKey(event.target.value)} />
          </label>}
          {isOAuth && !editing && <div className="subscription-oauth">
            <small className="form-hint">{oauthCopy.hint}</small>
          </div>}
        </div>
        {error && <small className="subscription-form-error" role="alert">{error}</small>}
        <div className="modal-actions">
          <button className="button button-muted" type="button" onClick={onClose}>取消</button>
          <button className="button button-primary" type="submit" disabled={saving || !name.trim() || (!isOAuth && !editing && !key.trim()) || (needsUrl && !baseUrl.trim())}>{saving ? (isOAuth && !editing ? oauthCopy.waiting : "保存中…") : isOAuth && !editing ? oauthCopy.action : editing ? "保存" : "添加"}</button>
        </div>
      </form>
    </div>
  </div>;
}
