I want you to implement a persistent project memory and context management system for this entire project.

My local LLM has a limited context window. When the context fills up, I currently have to start a new conversation and explain the project again. I want to eliminate that problem as much as possible.

Please:

1. Inspect the entire project and understand its architecture.
2. Create an AGENTS.md with permanent project instructions.
3. Create PROJECT_STATUS.md to track current development progress.
4. Create TASKS.md to track completed and outstanding work.
5. Create DECISIONS.md to preserve important technical decisions.
6. Create CHANGELOG.md to record modifications.
7. Establish a checkpoint system that saves progress automatically at meaningful milestones.
8. Before context compaction or when approaching the context limit, save a comprehensive checkpoint.
9. When a new conversation starts, automatically read the project memory files and resume where development stopped.
10. Never assume that conversation history is permanent memory.
11. Never overwrite existing project documentation without reviewing it first.
12. Keep memory files concise, accurate and updated as development progresses.

IMPORTANT:
Do not simply create empty documentation files. Analyze the existing codebase and populate them with actual project information.

I want to be able to close VS Code, restart my computer, start a completely new AI conversation, and have the AI understand exactly where the project stands and continue working.

Implement this system and verify that it works.