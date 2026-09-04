/** Free function at module scope. */
export function freeFunction(): number {
  const x = 1;
  return x;
}

/** Returns a greeting for the given name. */
export function documented(name: string): string {
  return `hello ${name}`;
}

export class Tracker {
  value: number = 0;

  constructor() {
    this.value = 0;
  }

  update(): void {
    this.value += 1;
  }
}

export function itWorks(): void {
  // test-like
}
