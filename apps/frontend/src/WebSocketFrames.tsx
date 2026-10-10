import "./websocket.css";
import { useState } from "react";
import { t } from "./i18n";
import { invoke } from "./api";
import { encodeText, type Flow } from "./types";

export default function WebSocketFrames({
  flow,
  onCopy,
  messageDraft,
  onMessageDraftChange,
}: {
  messageDraft?: { body: string; opcode: string };
  onMessageDraftChange?: (
    patch: Partial<{ body: string; opcode: string }>,
  ) => void;
  flow: Flow;
  onCopy: (value: string) => void;
}) {
  const [messages, setMessages] = useState(false);
  const [mode, setMode] = useState("auto");
  const [direction, setDirection] = useState("all");
  const [error, setError] = useState("");
  const [localDraft, setLocalDraft] = useState({ body: "", opcode: "1" });
  const { body, opcode } = messageDraft ?? localDraft;
  const change = (patch: Partial<typeof localDraft>) => {
    if (onMessageDraftChange) onMessageDraftChange(patch);
    else setLocalDraft((v) => ({ ...v, ...patch }));
  };
  const setBody = (body: string) => change({ body });
  const setOpcode = (opcode: string) => change({ opcode });
  const [sending, setSending] = useState(false);
  const [closing, setClosing] = useState(false);
  async function sendMessage(code = Number(opcode)) {
    setSending(true);
    setError("");
    try {
      await invoke("websocket_send", {
        id: flow.id,
        opcode: code,
        body: code === 8 ? "" : opcode === "1" ? encodeText(body) : body,
      });
      if (code === 8) setClosing(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setSending(false);
    }
  }
  function editMessage(frame: {
    opcode: number;
    payloadBase64: string;
    compressed: boolean;
    fin: boolean;
  }) {
    try {
      if (frame.opcode === 1 && !frame.compressed && frame.fin) {
        setBody(
          new TextDecoder("utf-8", { fatal: true }).decode(
            Uint8Array.from(atob(frame.payloadBase64), (c) => c.charCodeAt(0)),
          ),
        );
        setOpcode("1");
      } else {
        setBody(frame.payloadBase64);
        setOpcode("2");
      }
      setError("");
    } catch (e) {
      setError(String(e));
    }
  }
  const session = flow.websocket;
  if (!session) return null;
  const names: Record<number, string> = {
    0: "Continuation",
    1: "Text",
    2: "Binary",
    8: "Close",
    9: "Ping",
    10: "Pong",
  };
  function payload(frame: NonNullable<Flow["websocket"]>["frames"][number]) {
    if (mode === "base64") return frame.payloadBase64;
    const bytes = Uint8Array.from(atob(frame.payloadBase64), (c) =>
      c.charCodeAt(0),
    );
    if (
      mode === "auto" &&
      !frame.compressed &&
      frame.opcode === 1 &&
      frame.fin
    ) {
      try {
        return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      } catch {
        /* show bytes */
      }
    }
    return Array.from(
      bytes.slice(0, 65536),
      (v, i) =>
        `${i && i % 16 === 0 ? "\n" : i ? " " : ""}${v.toString(16).padStart(2, "0")}`,
    ).join("");
  }
  return (
    <div className="formatted-response ws-panel">
      <div className="ws-toolbar">
        <label>
          <input
            type="checkbox"
            checked={messages}
            disabled={!session.messages?.length}
            onChange={(e) => setMessages(e.target.checked)}
          />
          {t("重组消息")}
        </label>
        <strong>
          WebSocket · {session.state} · {session.frames.length}
        </strong>
        <select
          aria-label={t("消息方向")}
          value={direction}
          onChange={(e) => setDirection(e.target.value)}
        >
          <option value="all">{t("全部")}</option>
          <option value="client">{t("客户端 → 服务端")}</option>
          <option value="server">{t("服务端 → 客户端")}</option>
        </select>
        <select
          aria-label={t("消息格式")}
          value={mode}
          onChange={(e) => setMode(e.target.value)}
        >
          <option value="auto">{t("自动识别")}</option>
          <option value="hex">Hex</option>
          <option value="base64">Base64</option>
        </select>
        {["connecting", "open"].includes(session.state) && (
          <button
            onClick={() =>
              void invoke("cancel_replay", { executionId: flow.id }).catch(
                (e) => setError(String(e)),
              )
            }
          >
            {t("停止连接")}
          </button>
        )}
      </div>
      {flow.source === "replay" && (
        <div className="ws-composer">
          <p>
            {t(
              "主动 WebSocket：编辑消息后点击发送。文本使用 UTF-8，二进制及 Ping 使用 Base64。Ping 自动回复 Pong，关闭时等待服务端回应。",
            )}
          </p>
          <div className="ws-compose-toolbar">
            <select
              aria-label={t("发送消息类型")}
              value={opcode}
              onChange={(e) => setOpcode(e.target.value)}
            >
              <option value="1">Text · UTF-8</option>
              <option value="2">Binary · Base64</option>
              <option value="9">Ping · Base64</option>
            </select>
            <button
              className="ws-send"
              disabled={sending || closing || session.state !== "open"}
              onClick={() => void sendMessage()}
            >
              {t("发送消息")}
            </button>
            <button
              disabled={sending || closing || session.state !== "open"}
              onClick={() => void sendMessage(8)}
            >
              {t("正常关闭")}
            </button>
          </div>
          <textarea
            aria-label={t("WebSocket 消息正文")}
            value={body}
            onChange={(e) => setBody(e.target.value)}
            spellCheck={false}
          />
        </div>
      )}
      {error && <div className="error-box">{error}</div>}
      <div className="ws-note">
        {messages
          ? t(
              "重组完整消息并解压 permessage-deflate；原始帧保持不变，解码错误单独显示。",
            )
          : t(
              "逐帧展示；压缩及分片载荷以 Hex/Base64 查看，Hex 最多显示前 64 KiB。",
            )}
      </div>
      <div className="formatted-scroll ws-messages">
        {(messages
          ? session.messages!.map((m) => ({
              ...m,
              fin: true,
              compressed: false,
            }))
          : session.frames
        ).map((frame, i) =>
          direction !== "all" && frame.direction !== direction ? null : (
            <details className={"ws-message ws-" + frame.direction} key={i}>
              <summary>
                <span className="ws-time">
                  {(frame.atMs / 1000).toFixed(3)} s
                </span>
                <span className="ws-direction">
                  {frame.direction === "client" ? "↑" : "↓"}
                </span>
                <span className="ws-kind">
                  {names[frame.opcode] ?? frame.opcode}
                </span>
                <span className="ws-preview">
                  {"error" in frame && frame.error
                    ? String(frame.error)
                    : payload(frame).slice(0, 240) || "—"}
                </span>
                <span className="ws-expand">›</span>
              </summary>
              {flow.source === "replay" &&
                [1, 2].includes(frame.opcode) &&
                frame.fin &&
                !frame.compressed &&
                (!("error" in frame) || !frame.error) && (
                  <button onClick={() => editMessage(frame)}>
                    {t("载入编辑")}
                  </button>
                )}
              <button
                className="text-button"
                onClick={() => onCopy(frame.payloadBase64)}
              >
                {t("复制")} Base64
              </button>
              <pre>
                {"error" in frame && frame.error
                  ? String(frame.error)
                  : payload(frame)}
              </pre>
            </details>
          ),
        )}
      </div>
    </div>
  );
}
