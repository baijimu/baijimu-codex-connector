import { createInterface } from "node:readline";

const rl = createInterface({ input: process.stdin });

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
    send({
      id: message.id,
      result: {
        data: [
          {
            id: "thr_listed",
            name: "Listed Thread",
            cwd: "/tmp/listed",
            source: "cli",
            preview: "Listed preview",
            gitInfo: null,
          },
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
        },
      },
    });
    return;
  }
  if (message.method === "turn/start") {
    send({
      method: "turn/started",
      params: {
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
        turn: {
          id: "turn_test",
        },
        status: "completed",
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
