import assert from 'node:assert/strict';
export function check({ trusted }) {
  trusted(['go','test','./...'], { 'config_contract_test.go': 'package fixture\nimport "testing"\nfunc TestConfigContract(t *testing.T) { t.Setenv("PORT", "9001"); var cfg Config = LoadConfig(); if cfg.Port != 9001 || Port() != 9001 { t.Fatal("configuration ignored") }; t.Setenv("PORT", "bad"); if LoadConfig().Port != 8080 { t.Fatal("invalid default") } }\n' });
}
