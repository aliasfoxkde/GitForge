import type { VercelRequest, VercelResponse } from '@vercel/node';

export default function handler(req: VercelRequest, res: VercelResponse) {
  const { name = 'World' } = req.query;
  res.json({
    message: `Hello ${name}!`,
    timestamp: new Date().toISOString(),
    version: '1.0.0',
  });
}
