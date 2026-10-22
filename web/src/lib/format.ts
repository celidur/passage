export const errorMessage = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Two-digit counters, e.g. `03`. */
export const pad = (value: number) => value.toString().padStart(2, '0');
