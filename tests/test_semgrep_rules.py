"""Exercise actual Rust rules on synthetic unsafe and safe examples (no compilation)."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SEMGREP = os.environ.get("SEMGREP") or shutil.which("semgrep")
RULES = Path(__file__).resolve().parents[1] / ".semgrep.yml"


@unittest.skipUnless(SEMGREP, "Semgrep is not installed")
class SemgrepRulesTests(unittest.TestCase):
    def scan(self, code):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "crates/fixture/src/lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text(code, encoding="utf-8")
            result = subprocess.run(
                [SEMGREP, "scan", "--oss-only", "--config", str(RULES),
                 "--json", "--metrics", "off", "--disable-version-check", "."],
                cwd=root, capture_output=True, text=True, timeout=60,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads(result.stdout)
            self.assertFalse(report["errors"], report["errors"])
            self.assertTrue(report["paths"]["scanned"], "No Rust files were scanned")
            return {finding["check_id"].split(".")[-1] for finding in report["results"]}

    def test_dangerous_constructs_trigger_every_rule(self):
        self.assertEqual(self.scan('''
use std::process::Command;
fn example(db: Db, client: Client) {
    let command = Command::new("bash");
    db.execute(&format!("DELETE FROM {}", table), []);
    unsafe { dangerous(); }
    client.danger_accept_invalid_certs(true);
}
'''), {"rust-shell-interpreter", "rust-formatted-sql", "rust-unsafe-block-review", "rust-disabled-tls-validation"})

    def test_direct_process_parameters_and_bound_sql_are_allowed(self):
        self.assertFalse(self.scan('''
use std::process::Command;
fn example(db: Db, value: String) {
    let command = Command::new("git").arg("status");
    db.execute("DELETE FROM notes WHERE id=?1", [value]);
    db.execute_batch(include_str!("migration.sql"));
}
'''))


if __name__ == "__main__":
    unittest.main()
