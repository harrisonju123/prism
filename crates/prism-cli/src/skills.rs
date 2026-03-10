use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub allowed_tools: Vec<String>,
    pub user_invocable: bool,
    pub content: String,
    pub dir: PathBuf,
}

impl Skill {
    /// Combine skill content with user-provided arguments into a prompt.
    pub fn expand(&self, args: &str) -> String {
        if args.is_empty() {
            self.content.clone()
        } else {
            format!("{}\n\nUser arguments: {args}", self.content)
        }
    }

    /// Read a companion file relative to this skill's directory.
    pub fn read_companion(&self, relative_path: &str) -> Option<String> {
        let path = self.dir.join(relative_path);
        std::fs::read_to_string(path).ok()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

impl SkillRegistry {
    /// Discover skills from user-level and project-level directories.
    /// Project-level skills override user-level skills with the same name.
    pub fn discover(start_dir: &Path) -> Self {
        let mut skills = HashMap::new();

        // 1. User-level: ~/.prism/skills/<name>/SKILL.md
        if let Some(home) = dirs::home_dir() {
            let user_skills = home.join(".prism").join("skills");
            discover_skills_in(&user_skills, &mut skills);
        }

        // 2. Project-level: walk up from start_dir to .git boundary
        let mut dir = start_dir.to_path_buf();
        let mut project_dirs: Vec<PathBuf> = Vec::new();
        loop {
            let skills_dir = dir.join(".prism").join("skills");
            if skills_dir.is_dir() {
                project_dirs.push(skills_dir);
            }
            let is_repo_root = dir.join(".git").exists();
            if is_repo_root || !dir.pop() {
                break;
            }
        }
        // Apply root-first so closer-to-cwd overrides
        project_dirs.reverse();
        for skills_dir in project_dirs {
            discover_skills_in(&skills_dir, &mut skills);
        }

        Self { skills }
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// Generate a system prompt section listing available skills.
    pub fn system_prompt_section(&self) -> String {
        let invocable: Vec<&Skill> = self
            .skills
            .values()
            .filter(|s| s.user_invocable)
            .collect();

        if invocable.is_empty() && self.skills.is_empty() {
            return String::new();
        }

        let mut out = String::from("\n\n## Skills\n\nAvailable skills (invoke via the `skill` tool):\n");
        let mut names: Vec<&String> = self.skills.keys().collect();
        names.sort();
        for name in names {
            let skill = &self.skills[name];
            let invocable_marker = if skill.user_invocable { " [user-invocable]" } else { "" };
            out.push_str(&format!("- **{}**: {}{}\n", name, skill.description, invocable_marker));
        }

        if !invocable.is_empty() {
            out.push_str("\nUser-invocable skills can also be triggered by the user with `/<skill-name>`.\n");
        }

        out
    }

    pub fn names(&self) -> Vec<&str> {
        self.skills.keys().map(|s| s.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

fn discover_skills_in(skills_dir: &Path, skills: &mut HashMap<String, Skill>) {
    let entries = match std::fs::read_dir(skills_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_file = path.join("SKILL.md");
        if let Some(skill) = parse_skill_md(&skill_file) {
            skills.insert(skill.name.clone(), skill);
        }
    }
}

fn parse_skill_md(path: &Path) -> Option<Skill> {
    let raw = std::fs::read_to_string(path).ok()?;
    let dir = path.parent()?.to_path_buf();
    let (frontmatter, content) = split_frontmatter(&raw)?;

    let mut name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();
    let mut description = String::new();
    let mut allowed_tools: Vec<String> = Vec::new();
    let mut user_invocable = false;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "name" => name = value.to_string(),
                "description" => description = value.to_string(),
                "user-invocable" => user_invocable = value == "true",
                "allowed-tools" => {
                    allowed_tools = value
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                _ => {}
            }
        }
    }

    if content.trim().is_empty() {
        return None;
    }

    Some(Skill {
        name,
        description,
        allowed_tools,
        user_invocable,
        content: content.trim().to_string(),
        dir,
    })
}

/// Split a markdown file into (frontmatter, body). Frontmatter is delimited by `---`.
fn split_frontmatter(raw: &str) -> Option<(String, String)> {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        // No frontmatter — treat entire content as body
        return Some((String::new(), raw.to_string()));
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let close_pos = after_first.find("\n---")?;
    let frontmatter = after_first[..close_pos].to_string();
    let body = after_first[close_pos + 4..].to_string();
    Some((frontmatter, body))
}

/// Parse a user invocation like "/commit fix login bug" into (skill_name, args).
pub fn parse_skill_invocation(input: &str) -> Option<(&str, &str)> {
    let input = input.trim();
    if !input.starts_with('/') {
        return None;
    }
    let without_slash = &input[1..];
    if without_slash.is_empty() {
        return None;
    }
    match without_slash.split_once(char::is_whitespace) {
        Some((name, args)) => Some((name, args.trim())),
        None => Some((without_slash, "")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_skill(dir: &Path, name: &str, content: &str) -> PathBuf {
        let skill_dir = dir.join(".prism").join("skills").join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(&skill_file, content).unwrap();
        skill_dir
    }

    #[test]
    fn parse_frontmatter_and_body() {
        let raw = "---\nname: hello\ndescription: Say hello\nuser-invocable: true\n---\nSay hello!";
        let (fm, body) = split_frontmatter(raw).unwrap();
        assert!(fm.contains("name: hello"));
        assert_eq!(body.trim(), "Say hello!");
    }

    #[test]
    fn parse_no_frontmatter() {
        let raw = "Just a body with no frontmatter.";
        let (fm, body) = split_frontmatter(raw).unwrap();
        assert!(fm.is_empty());
        assert_eq!(body.trim(), raw);
    }

    #[test]
    fn parse_skill_md_full() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("hello");
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(
            &skill_file,
            "---\nname: hello\ndescription: Say hello\nuser-invocable: true\nallowed-tools: bash, read_file\n---\nSay \"Hello, world!\" and nothing else.",
        ).unwrap();

        let skill = parse_skill_md(&skill_file).unwrap();
        assert_eq!(skill.name, "hello");
        assert_eq!(skill.description, "Say hello");
        assert!(skill.user_invocable);
        assert_eq!(skill.allowed_tools, vec!["bash", "read_file"]);
        assert_eq!(skill.content, "Say \"Hello, world!\" and nothing else.");
    }

    #[test]
    fn parse_skill_md_empty_body_returns_none() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("empty");
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(&skill_file, "---\nname: empty\n---\n   \n").unwrap();

        assert!(parse_skill_md(&skill_file).is_none());
    }

    #[test]
    fn discover_finds_project_skills() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        setup_skill(
            tmp.path(),
            "greet",
            "---\nname: greet\ndescription: Greet the user\nuser-invocable: true\n---\nSay hi!",
        );

        let registry = SkillRegistry::discover(tmp.path());
        assert!(!registry.is_empty());
        let skill = registry.get("greet").unwrap();
        assert_eq!(skill.description, "Greet the user");
        assert!(skill.user_invocable);
    }

    #[test]
    fn project_overrides_user_level() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();

        // Simulate user-level skill by creating it directly in the registry
        let mut skills = HashMap::new();
        skills.insert(
            "test".to_string(),
            Skill {
                name: "test".to_string(),
                description: "user-level".to_string(),
                allowed_tools: vec![],
                user_invocable: false,
                content: "user content".to_string(),
                dir: tmp.path().to_path_buf(),
            },
        );

        // Project-level skill with same name
        setup_skill(
            tmp.path(),
            "test",
            "---\nname: test\ndescription: project-level\n---\nproject content",
        );

        // Discover should find project-level
        let registry = SkillRegistry::discover(tmp.path());
        let skill = registry.get("test").unwrap();
        assert_eq!(skill.description, "project-level");
    }

    #[test]
    fn expand_with_args() {
        let skill = Skill {
            name: "commit".to_string(),
            description: "Commit changes".to_string(),
            allowed_tools: vec![],
            user_invocable: true,
            content: "Create a git commit.".to_string(),
            dir: PathBuf::from("/tmp"),
        };
        assert_eq!(skill.expand(""), "Create a git commit.");
        assert_eq!(
            skill.expand("fix login bug"),
            "Create a git commit.\n\nUser arguments: fix login bug"
        );
    }

    #[test]
    fn parse_skill_invocation_valid() {
        assert_eq!(
            parse_skill_invocation("/commit fix login bug"),
            Some(("commit", "fix login bug"))
        );
        assert_eq!(parse_skill_invocation("/hello"), Some(("hello", "")));
        assert_eq!(
            parse_skill_invocation("  /test  some args  "),
            Some(("test", "some args"))
        );
    }

    #[test]
    fn parse_skill_invocation_invalid() {
        assert!(parse_skill_invocation("no slash").is_none());
        assert!(parse_skill_invocation("/").is_none());
        assert!(parse_skill_invocation("").is_none());
    }

    #[test]
    fn system_prompt_section_empty_registry() {
        let registry = SkillRegistry::default();
        assert!(registry.system_prompt_section().is_empty());
    }

    #[test]
    fn system_prompt_section_lists_skills() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        setup_skill(
            tmp.path(),
            "hello",
            "---\nname: hello\ndescription: Say hello\nuser-invocable: true\n---\nSay hello!",
        );

        let registry = SkillRegistry::discover(tmp.path());
        let section = registry.system_prompt_section();
        assert!(section.contains("hello"));
        assert!(section.contains("Say hello"));
        assert!(section.contains("[user-invocable]"));
    }

    #[test]
    fn read_companion_file() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("myskill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("template.txt"), "hello template").unwrap();

        let skill = Skill {
            name: "myskill".to_string(),
            description: "test".to_string(),
            allowed_tools: vec![],
            user_invocable: false,
            content: "body".to_string(),
            dir: skill_dir,
        };

        assert_eq!(
            skill.read_companion("template.txt"),
            Some("hello template".to_string())
        );
        assert!(skill.read_companion("nonexistent.txt").is_none());
    }
}
