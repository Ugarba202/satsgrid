const createJiti = require("jiti");
const path = require("path");

const root = path.resolve(__dirname, "..");
const jiti = createJiti(root, {
  alias: {
    "@": path.resolve(root, "src")
  }
});

jiti("./tests/l402.test.ts");
jiti("./tests/session_closure.test.ts");
