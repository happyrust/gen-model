/** @type {import('next').NextConfig} */
const nextConfig = {
  eslint: {
    // 仅在开发环境忽略 ESLint 错误，生产环境严格检查
    ignoreDuringBuilds: process.env.NODE_ENV === 'development',
    dirs: ['app', 'components', 'lib', 'hooks', 'types'],
  },
  typescript: {
    // 仅在开发环境忽略 TypeScript 错误，生产环境严格检查
    ignoreBuildErrors: process.env.NODE_ENV === 'development',
  },
  images: {
    // 启用 Next.js 图片优化
    unoptimized: false,
    remotePatterns: [
      {
        protocol: 'https',
        hostname: '**',
      },
    ],
  },
  // 性能优化配置
  swcMinify: true,
  reactStrictMode: true,
}

export default nextConfig
