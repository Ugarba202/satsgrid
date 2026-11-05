import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  async rewrites() {
    return [
      {
        source: '/health',
        destination: 'http://127.0.0.1:8080/health',
      },
      {
        source: '/v1/:path*',
        destination: 'http://127.0.0.1:8080/v1/:path*',
      },
      {
        source: '/internal/:path*',
        destination: 'http://127.0.0.1:8080/internal/:path*',
      },
    ]
  },
};

export default nextConfig;

