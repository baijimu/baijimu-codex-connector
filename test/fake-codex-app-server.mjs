import { createInterface } from "node:readline";

const rl = createInterface({ input: process.stdin });
let latestThreadId = null;

process.on("SIGTERM", () => process.exit(0));
process.on("SIGINT", () => process.exit(0));

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

rl.on("line", (line) => {
  const message = JSON.parse(line);
  if (message.method === "initialize") {
    send({
      id: message.id,
      result: {
        userAgent: "fake-codex-app-server",
        platformFamily: "test",
        platformOs: "test",
      },
    });
    return;
  }
  if (message.method === "initialized") {
    return;
  }
  if (message.method === "thread/start") {
    latestThreadId = "thr_test";
    send({
      method: "thread/started",
      params: {
        thread: {
          id: "thr_test",
        },
      },
    });
    send({
      id: message.id,
      result: {
        thread: {
          id: "thr_test",
        },
      },
    });
    return;
  }
  if (message.method === "thread/list") {
    const wrapped = process.env.CODEX_FAKE_WRAP_THREAD_LIST === "1";
    const thread = {
      id: wrapped ? "thr_wrapped" : (latestThreadId || "thr_listed"),
      name: wrapped ? "Wrapped Thread" : "Listed Thread",
      cwd: wrapped ? "/tmp/wrapped" : "/tmp/listed",
      source: "cli",
      preview: wrapped ? "Wrapped preview" : "Listed preview",
      gitInfo: null,
      updatedAt: latestThreadId ? 101 : 100,
      status: {
        type: process.env.CODEX_FAKE_THREAD_ACTIVE === "1" ? "active" : "idle",
        ...(process.env.CODEX_FAKE_THREAD_ACTIVE === "1"
          ? { activeFlags: ["waitingOnApproval"] }
          : {}),
      },
      turns: [],
      requestParams: message.params,
    };
    send({
      id: message.id,
      result: {
        data: [
          wrapped ? {
            thread,
            wrapperMeta: "kept",
            requestParams: message.params,
          } : thread,
        ],
        nextCursor: null,
        backwardsCursor: "cursor_back",
      },
    });
    return;
  }
  if (message.method === "thread/search") {
    send({
      id: message.id,
      result: {
        data: [
          {
            thread: {
              id: "thr_search",
              name: "Search Thread",
              cwd: "/tmp/search",
            },
            matches: [],
          },
        ],
        nextCursor: null,
        backwardsCursor: null,
      },
    });
    return;
  }
  if (message.method === "thread/read") {
    send({
      id: message.id,
      result: {
        thread: {
          id: message.params.threadId,
          name: "Read Thread",
          cwd: "/tmp/read",
          turns: message.params.includeTurns ? [{ id: "turn_read", items: [] }] : [],
        },
      },
    });
    return;
  }
  if (message.method === "thread/turns/list") {
    if (process.env.CODEX_FAKE_DISABLE_TURNS_LIST === "1") {
      send({
        id: message.id,
        error: {
          code: -32601,
          message: "method not found",
        },
      });
      return;
    }
    send({
      id: message.id,
      result: {
        data: [
          {
            id: "turn_recent",
            items: [
              {
                id: "item_recent",
                type: "agent_message",
                text: `limit=${message.params.limit};direction=${message.params.sortDirection};items=${message.params.itemsView}`,
              },
            ],
          },
        ],
        nextCursor: message.params.cursor ? null : "older_cursor",
        backwardsCursor: "newer_cursor",
      },
    });
    return;
  }
  if (message.method === "app/list") {
    send({
      id: message.id,
      result: {
        data: [
          {
            id: "app_test",
            name: "Test App",
            isAccessible: true,
            isEnabled: true,
          },
        ],
        nextCursor: null,
      },
    });
    return;
  }
  if (message.method === "thread/resume") {
    send({
      id: message.id,
      result: {
        thread: {
          id: message.params.threadId,
          initialTurnsPage: message.params.initialTurnsPage ? {
            data: [
              {
                id: "turn_resume",
                items: [],
              },
            ],
            nextCursor: null,
            backwardsCursor: "resume_back",
          } : null,
        },
      },
    });
    return;
  }
  if (message.method === "turn/start") {
    latestThreadId = message.params.threadId;
    send({
      method: "turn/started",
      params: {
        threadId: message.params.threadId,
        turn: {
          id: "turn_test",
        },
      },
    });
    send({
      method: "item/agentMessage/delta",
      params: {
        threadId: message.params.threadId,
        turnId: "turn_test",
        delta: "hello",
      },
    });
    send({
      id: message.id,
      result: {
        turn: {
          id: "turn_test",
        },
      },
    });
    send({
      method: "turn/completed",
      params: {
        threadId: message.params.threadId,
        turn: {
          id: "turn_test",
          status: "completed",
          items: [],
          completedAt: 1786400000,
          durationMs: 25,
          error: null,
        },
      },
    });
    return;
  }
  if (message.method === "turn/steer" || message.method === "turn/interrupt") {
    send({
      id: message.id,
      result: {
        ok: true,
      },
    });
    return;
  }
  send({
    id: message.id,
    result: {
      method: message.method,
      params: message.params,
    },
  });
});
