import { z } from 'zod';

const envSchema = z.object({
  VITE_API_BASE_URL: z.string().optional(),
  MODE: z.enum(['development', 'production', 'test']).default('development'),
});

const env = envSchema.parse(import.meta.env);

const API_BASE_URL = env.VITE_API_BASE_URL ?? '';

export const apiClient = {
  async get<T>(path: string): Promise<T> {
    const url = `${API_BASE_URL}${path}`;
    const response = await fetch(url);
    if (!response.ok) {
      throw new Error(`API error: ${response.status} ${response.statusText}`);
    }
    return response.json();
  },

  async post<T, D = unknown>(path: string, data: D): Promise<T> {
    const url = `${API_BASE_URL}${path}`;
    const response = await fetch(url, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(data),
    });
    if (!response.ok) {
      throw new Error(`API error: ${response.status} ${response.statusText}`);
    }
    return response.json();
  },
};

export { z };
export { env };
