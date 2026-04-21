/** @type {import('tailwindcss').Config} */
export default {
    content: [
        './index.html',
        './src/**/*.{vue,js,ts,jsx,tsx}',
    ],
    theme: {
        extend: {
            colors: {
                brand: {
                    500: '#3b82f6',
                    400: '#60a5fa',
                },
            },
        },
    },
    plugins: [],
};
