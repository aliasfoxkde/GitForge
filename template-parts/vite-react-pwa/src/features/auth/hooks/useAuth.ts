import { useAuthStore } from '../stores/authStore';

export function useAuth() {
  const { user, isLoading, login, logout } = useAuthStore();

  return {
    user,
    isLoading,
    isAuthenticated: !!user,
    login,
    logout,
  };
}
