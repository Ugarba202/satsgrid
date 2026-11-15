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
jiti("./tests/session_lifecycle.test.ts");
jiti("./tests/inference_session.test.ts");
jiti("./tests/session_read.test.ts");
jiti("./tests/lightning_invoice.test.ts");
jiti("./tests/exhausted_session_recovery.test.ts");
jiti("./tests/receipt_experience.test.ts");
jiti("./tests/accessibility_voice.test.ts");
jiti("./tests/receipt_verifier.test.ts");
jiti("./tests/receipt_public_verification.test.ts");
jiti("./tests/pricing_estimator.test.ts");
jiti("./tests/operator_accounting.test.ts");
