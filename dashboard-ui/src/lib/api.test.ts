import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, api } from "./api";

/**
 * Fehlertext-Verhalten von request(): Backend liefert bei 400/409
 * `{"error":"bad request"|"conflict", "detail":"<konkrete Meldung>"}` —
 * die detail-meldung soll in ApiError.message landen (Toasts), sonst
 * error, dann statusText.
 */

/** Minimaler Response-Doppel: nur die Member, die request() anfasst. */
function fakeResponse(status: number, statusText: string, body: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText,
    json: async () => body,
  } as unknown as Response;
}

function stubFetch(body: unknown, status = 200, statusText = "OK") {
  vi.stubGlobal("fetch", vi.fn(async () => fakeResponse(status, statusText, body)));
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("request — Fehlertext (ApiError.message)", () => {
  it("bevorzugt detail vor error", async () => {
    stubFetch({ error: "bad request", detail: "model 'x' existiert nicht" }, 400, "Bad Request");
    await expect(api.get("/redirects")).rejects.toMatchObject({
      status: 400,
      message: "model 'x' existiert nicht",
    });
  });

  it("faellt ohne detail auf error zurueck", async () => {
    stubFetch({ error: "conflict" }, 409, "Conflict");
    await expect(api.get("/redirects")).rejects.toMatchObject({
      status: 409,
      message: "conflict",
    });
  });

  it("faellt ohne detail und error auf statusText zurueck", async () => {
    stubFetch({ foo: "bar" }, 500, "Internal Server Error");
    await expect(api.get("/redirects")).rejects.toMatchObject({
      status: 500,
      message: "Internal Server Error",
    });
  });

  it("wirft ApiError mit status", async () => {
    stubFetch({ error: "bad request", detail: "detail-text" }, 400, "Bad Request");
    const err = await api.get("/redirects").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
  });
});
