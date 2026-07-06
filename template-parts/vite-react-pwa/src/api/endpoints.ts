import { apiClient } from './client';

export const api = {
  auth: {
    login: (email: string, password: string) =>
      apiClient.post<{ token: string; user: User }>('/auth/login', { email, password }),
    logout: () => apiClient.post<void>('/auth/logout'),
    me: () => apiClient.get<User>('/auth/me'),
  },
  users: {
    list: (params?: { page?: number; limit?: number }) =>
      apiClient.get<User[]>('/users', { params }),
    get: (id: string) => apiClient.get<User>(`/users/${id}`),
    create: (data: CreateUserData) => apiClient.post<User>('/users', data),
    update: (id: string, data: UpdateUserData) => apiClient.patch<User>(`/users/${id}`, data),
    delete: (id: string) => apiClient.delete<void>(`/users/${id}`),
  },
};

export interface User {
  id: string;
  name: string;
  email: string;
  createdAt: string;
  updatedAt: string;
}

export interface CreateUserData {
  name: string;
  email: string;
  password: string;
}

export interface UpdateUserData {
  name?: string;
  email?: string;
  password?: string;
}
