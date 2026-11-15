var headers = new Headers({ "X-QT-AUTH": "token", "Accept": "application/json" });
if (headers.get("x-qt-auth") !== "token") throw new Error("case " + headers.get("x-qt-auth"));
var request = new Request("https://example.com/api", {
  headers: headers,
  credentials: "include"
});
if (request.credentials !== "include") throw new Error("credentials " + request.credentials);
if (request.headers.get("X-QT-AUTH") !== "token") throw new Error("forward " + request.headers.get("X-QT-AUTH"));
var plain = new Request("/me", { headers: { "X-Request-Id": "9" } });
if (plain.headers.get("x-request-id") !== "9") throw new Error("plain");
if (plain.credentials !== "same-origin") throw new Error("default credentials");
