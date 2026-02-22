// Bull Board Setup
// Monitoring dashboard for BullMQ queues using Bull Board

import { INestApplication } from '@nestjs/common';
import { Queue } from 'bullmq';
import { createBullBoard } from '@bull-board/api';
import { ExpressAdapter } from '@bull-board/express';

/**
 * Configure Bull Board monitoring dashboard
 * Accessible at /admin/queues
 */
export function setupBullBoard(
  app: INestApplication,
  queues: Queue[],
): void {
  const serverAdapter = new ExpressAdapter();
  serverAdapter.setBasePath('/admin/queues');

  createBullBoard({
    queues: queues.map((queue) => ({
      name: queue.name,
      client: queue as any,
    })),
    serverAdapter,
  });

  app.use('/admin/queues', serverAdapter.getRouter());
}
