export type HealthStatus = 'healthy' | 'degraded' | 'unhealthy';

export interface HealthResponse {
	status: HealthStatus;
	timestamp: string;
	version: string;
}

export interface Resource {
	id: string;
	name: string;
	description?: string;
	tags?: string[];
	createdAt: string;
	updatedAt: string;
	metadata?: Record<string, unknown>;
}

export interface CreateResourceRequest {
	name: string;
	description?: string;
	tags?: string[];
	metadata?: Record<string, unknown>;
}

export interface UpdateResourceRequest {
	name?: string;
	description?: string;
	tags?: string[];
	metadata?: Record<string, unknown>;
}

export interface ApiResponse<T = unknown> {
	success: boolean;
	data?: T;
	error?: string;
}

export interface PaginationParams {
	page: number;
	pageSize: number;
}

export interface PaginatedResponse<T> {
	items: T[];
	total: number;
	page: number;
	pageSize: number;
	totalPages: number;
}
