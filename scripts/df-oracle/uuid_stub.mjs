// Deterministic stand-in for the `uuid` package inside the DragonFruit oracle
// sandbox: `v4()` returns `id-1`, `id-2`, ... so DragonFruit's entities can be
// compared field for field with the native port, which numbers its ids the same.
let counter = 0;

export function v4() {
    counter += 1;
    return `id-${counter}`;
}

export function __resetUuid() {
    counter = 0;
}

export default { v4, __resetUuid };
