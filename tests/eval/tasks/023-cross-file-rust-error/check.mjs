import assert from 'node:assert/strict';
export function check({ trusted }) {
  trusted(['cargo','test','--offline','--locked'], { 'tests/shared_error.rs': '#[test] fn shared() { let _: Result<String, fixture::error::AppError> = fixture::reader::read(""); let _: Result<String, fixture::error::AppError> = fixture::writer::write(""); }\n' });
}
