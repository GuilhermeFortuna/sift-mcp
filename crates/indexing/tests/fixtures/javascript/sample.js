/** Free function at module scope. */
export function freeFunction() {
  const x = 1;
  return x;
}

/** Returns a greeting for the given name. */
export function documented(name) {
  return `hello ${name}`;
}

export class Tracker {
  constructor() {
    this.value = 0;
  }

  update() {
    this.value += 1;
  }
}

export function itWorks() {
  return true;
}
