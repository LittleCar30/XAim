import assert from 'node:assert/strict';
export function check({ trusted }) {
  trusted(['python','-c', 'import models, service, serializer; from dataclasses import is_dataclass; assert is_dataclass(models.User); assert service.User is models.User; assert serializer.User is models.User; assert service.greet(models.User("B"))=="Hello B"; assert serializer.serialize(models.User("B"))=={"name":"B"}'], {});
}
