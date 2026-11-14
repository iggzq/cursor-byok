import { useEffect, useState } from "react";
import { api, type OverviewMode, type OverviewSource, type OverviewSyncSettings, type OverviewSyncStatus } from "../../shared/api";
import { appStore } from "../../shared/store/appStore";
import { Button } from "../../shared/ui/Button";
import { ConfirmDialog } from "../../shared/ui/ConfirmDialog";
import { FormField, SecretTextInput, TextInput } from "../../shared/ui/FormControls";
import { Select } from "../../shared/ui/Select";
import { TitledCard } from "../../shared/ui/TitledCard";
import { useMessage } from "../../shared/ui/message";
import styles from "./OverviewSyncSettingsCard.module.scss";

const DEFAULT_SYNC: OverviewSyncSettings = {
  mode: "local",
  center_url: "",
  api_key: "",
  device_id: "",
  device_name: "",
};

function formatTime(value: number | null) {
  if (!value) return t("尚未同步");
  return new Date(value).toLocaleString();
}

function normalizeUrl(value: string) {
  const trimmed = value.trim();
  if (!trimmed) return "";
  return /^[a-z]+:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`;
}

export function OverviewSyncSettingsCard() {
  const message = useMessage();
  const [saved, setSaved] = useState<OverviewSyncSettings | null>(null);
  const [draft, setDraft] = useState<OverviewSyncSettings>(DEFAULT_SYNC);
  const [status, setStatus] = useState<OverviewSyncStatus | null>(null);
  const [sources, setSources] = useState<OverviewSource[]>([]);
  const [saving, setSaving] = useState(false);
  const [flushing, setFlushing] = useState(false);
  const [testing, setTesting] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);

  const loadStatus = async () => {
    const [nextStatus, nextSources] = await Promise.all([
      api.overviewSyncStatus(),
      api.overviewSources().catch(() => [] as OverviewSource[]),
    ]);
    setStatus(nextStatus);
    setSources(nextSources);
  };

  useEffect(() => {
    void (async () => {
      const sync = await api.overviewSyncSettings();
      setSaved(sync);
      setDraft(sync);
      await loadStatus();
    })().catch((cause: unknown) => message(cause instanceof Error ? cause.message : String(cause)));
  }, [message]);

  const remote = draft.mode === "remote";
  const dirty = saved !== null && (
    saved.mode !== draft.mode
    || saved.center_url !== draft.center_url
    || saved.api_key !== draft.api_key
    || saved.device_name !== draft.device_name
  );

  const save = async () => {
    try {
      setSaving(true);
      const next = await appStore.updateOverviewSync({
        ...draft,
        center_url: normalizeUrl(draft.center_url),
        device_name: draft.device_name.trim(),
      });
      setSaved(next);
      setDraft(next);
      await appStore.refresh();
      await loadStatus();
      message(t("概览数据设置已保存"));
    } catch (cause) {
      message(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  };

  const flush = async () => {
    try {
      setFlushing(true);
      setStatus(await api.flushOverviewSync());
      await appStore.refresh();
      await loadStatus();
      message(t("已完成一次远程同步"));
    } catch (cause) {
      message(cause instanceof Error ? cause.message : String(cause));
      void loadStatus().catch(() => undefined);
    } finally {
      setFlushing(false);
    }
  };

  const test = async () => {
    try {
      setTesting(true);
      await loadStatus();
      if (status?.last_error) {
        message(status.last_error);
      } else {
        message(t("已连接汇总中心（{count} 台设备）", { count: sources.length }));
      }
    } catch (cause) {
      message(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setTesting(false);
    }
  };

  const reset = async () => {
    try {
      setResetting(true);
      setStatus(await api.resetOverviewSync());
      setConfirmReset(false);
      await loadStatus();
      message(t("同步进度已重置，将重新上传本机历史记录"));
    } catch (cause) {
      message(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setResetting(false);
    }
  };

  return <TitledCard title={t("概览数据")} action={<Button size="small" variant="primary" disabled={!dirty || saving} onClick={() => void save()}>
    {saving ? t("保存中…") : t("保存")}
  </Button>}>
    <div className={styles.content}>
      <div className={styles.row}>
        <div className={styles.description}>
          <strong>{t("数据来源")}</strong>
          <small>{t("本地模式只统计本机调用；远程汇总模式从独立部署的汇总中心读取各平台数据。")}</small>
        </div>
        <div className={styles.modeControl}>
          <Select
            value={draft.mode}
            ariaLabel={t("数据来源")}
            options={[
              { value: "local", label: t("本地模式") },
              { value: "remote", label: t("远程汇总模式") },
            ]}
            onChange={(value) => setDraft((current) => ({ ...current, mode: value as OverviewMode }))}
          />
        </div>
      </div>

      {remote && <>
        <FormField label={t("汇总中心地址")} hint={t("中心服务根地址，例如 http://10.0.0.5:4399，末尾不需要路径。")}>
          <TextInput
            value={draft.center_url}
            placeholder="http://10.0.0.5:4399"
            autoComplete="off"
            disabled={saving}
            onChange={(event) => setDraft((current) => ({ ...current, center_url: event.target.value }))}
          />
        </FormField>
        <FormField label={t("汇总密钥")} hint={t("必须与中心服务启动时配置的 CURSOR_OVERVIEW_COLLECTOR_API_KEY 一致。")}>
          <SecretTextInput
            value={draft.api_key}
            autoComplete="off"
            disabled={saving}
            onChange={(event) => setDraft((current) => ({ ...current, api_key: event.target.value }))}
          />
        </FormField>
        <FormField label={t("设备名称")} hint={t("用于在多台设备之间区分数据来源。")}>
          <TextInput
            value={draft.device_name}
            placeholder={t("例如 MacBook")}
            autoComplete="off"
            disabled={saving}
            onChange={(event) => setDraft((current) => ({ ...current, device_name: event.target.value }))}
          />
        </FormField>
        <div className={styles.status}>
          <div>
            <strong>{t("同步状态")}</strong>
            <small>
              {status
                ? t("待推送 {pending} 条 · 最近成功 {time}", { pending: status.pending_calls, time: formatTime(status.last_push_at_ms) })
                : t("读取中…")}
            </small>
            {sources.length > 0 && <small>{t("中心已收到 {count} 台设备的数据", { count: sources.length })}</small>}
            {status?.last_error && <small className={styles.error}>{status.last_error}</small>}
          </div>
          <div className={styles.statusActions}>
            <Button size="small" disabled={testing || saving || !saved} onClick={() => void test()}>
              {testing ? t("检测中…") : t("检测连接")}
            </Button>
            <Button size="small" disabled={flushing || saving || !saved || !remote} onClick={() => void flush()}>
              {flushing ? t("同步中…") : t("立即同步")}
            </Button>
          </div>
        </div>
        {remote && <div className={styles.resetRow}>
          <div>
            <strong>{t("重新上传历史")}</strong>
            <small>{t("清空同步进度后重传本机全部历史记录；中心按调用 ID 去重，不会产生重复数据。切换中心或中心数据被清空后使用。")}</small>
          </div>
          <Button size="small" disabled={resetting || saving || flushing || !saved} onClick={() => setConfirmReset(true)}>
            {resetting ? t("重置中…") : t("重新上传")}
          </Button>
        </div>}
        {sources.length > 0 && <div className={styles.sources}>
          {sources.map((source) => <div key={source.source_id} className={styles.source}>
            <span>{source.name}</span>
            <small>{t("{calls} 条 · 最近 {time}", { calls: source.call_count, time: formatTime(source.last_push_at_ms) })}</small>
          </div>)}
        </div>}
      </>}

      <div className={styles.divider} />
      <div className={styles.description}>
        <strong>{t("部署汇总中心")}</strong>
        <small>{t("汇总中心是独立服务，部署在中心服务器上，与桌面应用互不影响：")}</small>
        <code className={styles.command}>CURSOR_OVERVIEW_COLLECTOR_API_KEY=&lt;密钥&gt; cursor-overview-collector</code>
        <small>{t("默认监听 0.0.0.0:4399，可用 CURSOR_OVERVIEW_COLLECTOR_ADDR 与 CURSOR_DATABASE_URL 调整。桌面端永远只作为数据来源，不会开放外部端口。")}</small>
      </div>
    </div>
    <ConfirmDialog
      open={confirmReset}
      title={t("重新上传全部历史记录？")}
      busy={resetting}
      cancelLabel={t("取消")}
      confirmLabel={t("开始重新上传")}
      onCancel={() => setConfirmReset(false)}
      onConfirm={() => void reset()}
    >
      <div className={styles.confirmContent}>
        {t("将清空本机同步进度并重新上传全部历史调用记录。中心按调用 ID 去重，已有数据不会重复；数据量较大时上传需要一段时间。")}
      </div>
    </ConfirmDialog>
  </TitledCard>;
}