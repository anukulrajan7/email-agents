//! Skill taxonomy: aliases → one canonical name ("ReactJS" → "react", "k8s" → "kubernetes"), free-text skill
//! detection (longest alias wins, so "Spring Boot" is not also "spring"), and related skills ("spring boot" is
//! related to "spring") that earn half credit when matching. Applied at extraction and at scoring.

use std::{
    collections::{BTreeSet, HashMap},
    sync::OnceLock,
};

/// Canonical name (lower case) and every spelling we accept for it. Order is the display order of
/// skills found in free text.
pub const TAXONOMY: &[(&str, &[&str])] = &[
    ("java", &["java", "core java", "j2ee", "java ee", "jee"]),
    ("spring boot", &["spring boot", "springboot", "spring-boot"]),
    ("spring", &["spring", "spring framework", "spring mvc"]),
    ("microservices", &["microservices", "micro services", "micro-services", "microservice"]),
    ("hibernate", &["hibernate", "jpa"]),
    ("kafka", &["kafka", "apache kafka"]),
    ("python", &["python", "python3"]),
    ("django", &["django"]),
    ("flask", &["flask"]),
    ("fastapi", &["fastapi", "fast api"]),
    ("javascript", &["javascript", "js", "es6", "ecmascript"]),
    ("typescript", &["typescript", "ts"]),
    ("react", &["react", "reactjs", "react.js", "react js"]),
    ("next.js", &["next.js", "nextjs", "next js"]),
    ("react native", &["react native", "react-native"]),
    ("angular", &["angular", "angularjs", "angular.js"]),
    ("vue", &["vue", "vuejs", "vue.js"]),
    ("node.js", &["node.js", "nodejs", "node", "node js"]),
    ("express", &["express", "expressjs", "express.js"]),
    ("nestjs", &["nestjs", "nest.js"]),
    ("html", &["html", "html5"]),
    ("css", &["css", "css3", "scss", "sass"]),
    ("tailwind", &["tailwind", "tailwindcss", "tailwind css"]),
    (".net", &[".net", "dotnet", "dot net", ".net core", "dotnet core"]),
    ("c#", &["c#", "csharp", "c sharp"]),
    ("asp.net", &["asp.net", "asp.net mvc", "asp.net core"]),
    ("c++", &["c++", "cpp"]),
    ("go", &["go", "golang", "go lang"]),
    ("rust", &["rust"]),
    ("php", &["php"]),
    ("laravel", &["laravel"]),
    ("ruby", &["ruby"]),
    ("ruby on rails", &["ruby on rails", "rails", "ror"]),
    ("scala", &["scala"]),
    ("kotlin", &["kotlin"]),
    ("swift", &["swift"]),
    ("sql", &["sql"]),
    ("pl/sql", &["pl/sql", "plsql"]),
    ("mysql", &["mysql"]),
    ("postgresql", &["postgresql", "postgres", "psql"]),
    ("sql server", &["sql server", "ms sql", "mssql", "ms sql server", "microsoft sql server", "t-sql", "tsql"]),
    ("oracle", &["oracle", "oracle db"]),
    ("mongodb", &["mongodb", "mongo"]),
    ("redis", &["redis"]),
    ("elasticsearch", &["elasticsearch", "elastic search"]),
    ("graphql", &["graphql"]),
    ("rest api", &["rest api", "rest apis", "restful", "rest"]),
    ("aws", &["aws", "amazon web services"]),
    ("azure", &["azure", "microsoft azure"]),
    ("gcp", &["gcp", "google cloud", "google cloud platform"]),
    ("devops", &["devops", "dev ops"]),
    ("ci/cd", &["ci/cd", "cicd", "ci cd"]),
    ("jenkins", &["jenkins"]),
    ("terraform", &["terraform"]),
    ("kubernetes", &["kubernetes", "k8s"]),
    ("docker", &["docker"]),
    ("linux", &["linux", "unix"]),
    ("git", &["git", "github", "gitlab"]),
    ("salesforce", &["salesforce", "sfdc"]),
    ("sap", &["sap"]),
    ("sap abap", &["sap abap", "abap"]),
    ("sap fico", &["sap fico", "fico"]),
    ("sap mm", &["sap mm"]),
    ("testing", &["testing", "qa", "manual testing", "quality assurance"]),
    ("automation testing", &["automation testing", "test automation"]),
    ("selenium", &["selenium"]),
    ("data engineering", &["data engineering", "data engineer", "etl"]),
    ("spark", &["spark", "apache spark"]),
    ("pyspark", &["pyspark"]),
    ("hadoop", &["hadoop"]),
    ("airflow", &["airflow", "apache airflow"]),
    ("power bi", &["power bi", "powerbi"]),
    ("tableau", &["tableau"]),
    ("machine learning", &["machine learning", "ml", "ai/ml"]),
    ("deep learning", &["deep learning"]),
    ("tensorflow", &["tensorflow"]),
    ("pytorch", &["pytorch"]),
    ("nlp", &["nlp", "natural language processing"]),
    ("android", &["android"]),
    ("ios", &["ios"]),
    ("flutter", &["flutter"]),
    ("dart", &["dart"]),
];

/// Aliases too ambiguous to spot in prose ("let's go", "the rest", "express interest"). They still
/// normalise when they arrive as a skill field.
const NOT_IN_PROSE: &[&str] = &["go", "ts", "rest", "express"];

/// Specific skill ⇒ the broader skills it implies. Used in both directions: a candidate with either side
/// gets half credit for the other.
pub const RELATED: &[(&str, &[&str])] = &[
    ("spring boot", &["spring", "java"]),
    ("spring", &["java"]),
    ("hibernate", &["java"]),
    ("next.js", &["react"]),
    ("react native", &["react"]),
    ("react", &["javascript"]),
    ("vue", &["javascript"]),
    ("angular", &["typescript"]),
    ("typescript", &["javascript"]),
    ("node.js", &["javascript"]),
    ("express", &["node.js"]),
    ("nestjs", &["node.js", "typescript"]),
    ("tailwind", &["css"]),
    ("django", &["python"]),
    ("flask", &["python"]),
    ("fastapi", &["python"]),
    ("pyspark", &["spark", "python"]),
    ("spark", &["data engineering"]),
    ("airflow", &["data engineering"]),
    ("hadoop", &["data engineering"]),
    ("mysql", &["sql"]),
    ("postgresql", &["sql"]),
    ("sql server", &["sql"]),
    ("oracle", &["sql"]),
    ("pl/sql", &["oracle", "sql"]),
    ("asp.net", &[".net", "c#"]),
    ("c#", &[".net"]),
    ("laravel", &["php"]),
    ("ruby on rails", &["ruby"]),
    ("kubernetes", &["docker", "devops"]),
    ("jenkins", &["ci/cd"]),
    ("ci/cd", &["devops"]),
    ("terraform", &["devops"]),
    ("deep learning", &["machine learning"]),
    ("tensorflow", &["deep learning", "machine learning"]),
    ("pytorch", &["deep learning", "machine learning"]),
    ("nlp", &["machine learning"]),
    ("selenium", &["automation testing"]),
    ("automation testing", &["testing"]),
    ("sap abap", &["sap"]),
    ("sap fico", &["sap"]),
    ("sap mm", &["sap"]),
    ("tableau", &["power bi"]),
    ("flutter", &["dart"]),
    ("swift", &["ios"]),
    ("kotlin", &["android"]),
];

fn alias_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| TAXONOMY.iter().flat_map(|(canon, aliases)| aliases.iter().map(move |a| (*a, *canon))).collect())
}

/// Every prose-safe alias with its taxonomy index, longest first.
fn prose_aliases() -> &'static [(usize, &'static str)] {
    static LIST: OnceLock<Vec<(usize, &'static str)>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut list: Vec<(usize, &'static str)> = TAXONOMY
            .iter()
            .enumerate()
            .flat_map(|(i, (_, aliases))| aliases.iter().map(move |a| (i, *a)))
            .filter(|(_, a)| !NOT_IN_PROSE.contains(a))
            .collect();
        list.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
        list
    })
}

fn related_map() -> &'static HashMap<&'static str, Vec<&'static str>> {
    static MAP: OnceLock<HashMap<&'static str, Vec<&'static str>>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        for (specific, broader) in RELATED {
            for b in *broader {
                map.entry(specific).or_default().push(b);
                map.entry(b).or_default().push(specific);
            }
        }
        map
    })
}

/// Byte offset of the first whole-word occurrence of `needle` (works for ".net", "c#", "c++").
pub fn find_word(haystack: &str, needle: &str) -> Option<usize> {
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let i = start + pos;
        let before = haystack[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = haystack[i + needle.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            return Some(i);
        }
        start = i + needle.len();
    }
    None
}

/// Lower-case canonical name; unknown skills are kept (trimmed, lower-cased, single-spaced).
pub fn canonical_skill(raw: &str) -> String {
    let s = raw.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let map = alias_map();
    if let Some(c) = map.get(s.as_str()) {
        return c.to_string();
    }
    let spaced = s.replace(['-', '_'], " ");
    map.get(spaced.as_str()).map(|c| c.to_string()).unwrap_or(s)
}

/// Canonical, de-duplicated, sorted.
pub fn canonical_skills(list: &[String]) -> Vec<String> {
    let set: BTreeSet<String> = list.iter().map(|s| canonical_skill(s)).filter(|s| !s.is_empty()).collect();
    set.into_iter().collect()
}

/// Skills mentioned in free text, in taxonomy order. Longer aliases are matched first and blanked out,
/// so "Spring Boot" yields only "spring boot" and "PL/SQL" only "pl/sql".
pub fn skills_in(text: &str) -> Vec<String> {
    let mut hay = text.to_lowercase();
    let mut found = BTreeSet::new();
    for (idx, alias) in prose_aliases() {
        while let Some(i) = find_word(&hay, alias) {
            found.insert(*idx);
            hay.replace_range(i..i + alias.len(), &" ".repeat(alias.len()));
        }
    }
    found.into_iter().map(|i| TAXONOMY[i].0.to_string()).collect()
}

/// Canonical skills directly related to `skill` (either direction).
pub fn related(skill: &str) -> &'static [&'static str] {
    related_map().get(skill).map(Vec::as_slice).unwrap_or(&[])
}

/// Every stored spelling that should count as overlapping with `skills`: each canonical skill, its related
/// skills, and all their aliases. Used to pre-select counterparts that share a skill.
pub fn overlap_terms(skills: &[String]) -> Vec<String> {
    let mut out = BTreeSet::new();
    for s in canonical_skills(skills) {
        let mut names = vec![s.clone()];
        names.extend(related(&s).iter().map(|r| r.to_string()));
        for name in names {
            if let Some((_, aliases)) = TAXONOMY.iter().find(|(c, _)| *c == name) {
                out.extend(aliases.iter().map(|a| a.to_string()));
            }
            out.insert(name);
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_map_to_one_canonical_name() {
        for (raw, canon) in [
            ("ReactJS", "react"),
            ("React.js", "react"),
            ("SpringBoot", "spring boot"),
            ("Spring-Boot", "spring boot"),
            ("node", "node.js"),
            ("NodeJS", "node.js"),
            ("K8s", "kubernetes"),
            ("JS", "javascript"),
            ("TS", "typescript"),
            ("Golang", "go"),
            ("Postgres", "postgresql"),
            ("PostgreSQL", "postgresql"),
            ("MS SQL", "sql server"),
            ("mssql", "sql server"),
            ("Sql  Server", "sql server"),
            ("dotnet", ".net"),
            (".NET", ".net"),
            ("ML", "machine learning"),
            ("Machine Learning", "machine learning"),
            ("Some_Unknown  Tool", "some_unknown tool"),
        ] {
            assert_eq!(canonical_skill(raw), canon, "{raw}");
        }
        assert_eq!(canonical_skills(&["ReactJS".into(), "react".into(), " ".into()]), ["react"]);
    }

    #[test]
    fn taxonomy_is_consistent() {
        assert!(TAXONOMY.len() >= 80);
        let mut seen = std::collections::HashSet::new();
        for (canon, aliases) in TAXONOMY {
            assert!(aliases.contains(canon), "{canon} lists itself");
            for a in *aliases {
                assert!(seen.insert(*a), "alias {a} is listed twice");
                assert_eq!(*a, a.to_lowercase());
            }
        }
        for (s, broader) in RELATED {
            for name in std::iter::once(s).chain(broader.iter()) {
                assert!(TAXONOMY.iter().any(|(c, _)| c == name), "{name} is not canonical");
            }
        }
    }

    #[test]
    fn prose_detection_prefers_longest_alias() {
        assert_eq!(skills_in("Java, Spring Boot and Microservices"), ["java", "spring boot", "microservices"]);
        assert_eq!(skills_in("Spring MVC, Hibernate"), ["spring", "hibernate"]);
        assert_eq!(skills_in("Strong PL/SQL and MS SQL Server"), ["pl/sql", "sql server"]);
        assert_eq!(skills_in("ReactJS, Next.js, Node.js"), ["react", "next.js", "node.js"]);
        assert!(skills_in("Let's go over the rest. Please express interest.").is_empty());
        assert_eq!(skills_in("knows .net and c#"), [".net", "c#"]);
        assert!(!skills_in("javascript").contains(&"java".to_string()));
    }

    #[test]
    fn related_skills_are_symmetric() {
        assert!(related("spring boot").contains(&"spring"));
        assert!(related("spring").contains(&"spring boot"));
        assert!(related("react").contains(&"next.js"));
        assert!(related("next.js").contains(&"react"));
        assert!(related("postgresql").contains(&"sql"));
        assert!(!related("postgresql").contains(&"mysql"), "siblings are not related");
        assert!(related("unknown skill").is_empty());
        let terms = overlap_terms(&["React".into()]);
        assert!(terms.contains(&"reactjs".into()) && terms.contains(&"nextjs".into()) && terms.contains(&"javascript".into()));
    }
}
